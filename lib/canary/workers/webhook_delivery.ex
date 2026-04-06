defmodule Canary.Workers.WebhookDelivery do
  @moduledoc """
  Oban worker for webhook delivery with retries, HMAC signing,
  circuit breaker checks, and cooldown enforcement.
  """

  use Oban.Worker,
    queue: :webhooks,
    max_attempts: 4,
    priority: 1

  alias Canary.Alerter.{CircuitBreaker, Cooldown, Signer}
  alias Canary.{Repo, WebhookDeliveries}
  alias Canary.Schemas.Webhook
  import Ecto.Query

  require Logger

  @delivery_timeout 10_000

  @impl Oban.Worker
  def perform(%Oban.Job{} = job) do
    %{"webhook_id" => webhook_id, "payload" => payload, "event" => event} = job.args
    now = DateTime.utc_now() |> DateTime.to_iso8601()
    delivery_id = delivery_id_from_job(job)
    WebhookDeliveries.create_pending(delivery_id, webhook_id, event, now)

    case Canary.Repos.read_repo().get(Webhook, webhook_id) do
      nil ->
        Logger.warning("Webhook #{webhook_id} not found, discarding")
        WebhookDeliveries.mark_discarded(delivery_id, "webhook_not_found", now)
        :ok

      %Webhook{active: 0} ->
        Logger.info("Webhook #{webhook_id} inactive, skipping")
        WebhookDeliveries.mark_discarded(delivery_id, "webhook_inactive", now)
        :ok

      webhook ->
        deliver(webhook, payload, event, delivery_id, job)
    end
  end

  def deliver_test(webhook, payload, event) do
    send_request(webhook, payload, event, Canary.ID.generate("DLV"), false, nil)
  end

  def deliver(webhook, payload, event, delivery_id, job) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    if CircuitBreaker.open?(webhook.id) and not CircuitBreaker.should_probe?(webhook.id) do
      Logger.info("Circuit open for #{webhook.id}, skipping")
      WebhookDeliveries.create_suppressed(delivery_id, webhook.id, event, "circuit_open", now)
      :ok
    else
      send_request(webhook, payload, event, delivery_id, true, job)
    end
  end

  @spec enqueue_for_event(String.t(), map()) :: :ok
  def enqueue_for_event(event, payload) do
    webhooks =
      from(w in Webhook, where: w.active == 1)
      |> Canary.Repos.read_repo().all()
      |> Enum.filter(&Webhook.subscribes_to?(&1, event))

    Enum.each(webhooks, fn webhook ->
      now = DateTime.utc_now() |> DateTime.to_iso8601()
      delivery_id = Canary.ID.generate("DLV")
      cooldown_key = cooldown_key(webhook.id, event, payload)

      if Cooldown.in_cooldown?(cooldown_key) do
        WebhookDeliveries.create_suppressed(delivery_id, webhook.id, event, "cooldown", now)
      else
        case enqueue_delivery(webhook, payload, event, delivery_id, now) do
          :ok ->
            Cooldown.mark(cooldown_key)

          {:error, reason} ->
            Logger.error("Failed to enqueue webhook delivery",
              webhook_id: webhook.id,
              event: event,
              reason: inspect(reason)
            )

            WebhookDeliveries.mark_discarded(delivery_id, "enqueue_failed", now)
        end
      end
    end)

    :ok
  end

  @impl Oban.Worker
  def backoff(%Oban.Job{attempt: attempt}) do
    case attempt do
      1 -> 1
      2 -> 5
      3 -> 30
      _ -> 60
    end
  end

  defp send_request(webhook, payload, event, delivery_id, track_circuit?, job) do
    body = Jason.encode!(payload)
    now = DateTime.utc_now() |> DateTime.to_iso8601()
    if track_circuit?, do: WebhookDeliveries.mark_attempt(delivery_id, now)

    headers = [
      {"content-type", "application/json"},
      {"x-signature", Signer.signature_header(body, webhook.secret)},
      {"x-event", event},
      {"x-delivery-id", delivery_id},
      {"x-webhook-version", "1"},
      {"x-sequence", to_string(payload["sequence"] || 0)}
    ]

    case Req.post(webhook.url,
           body: body,
           headers: headers,
           receive_timeout: @delivery_timeout,
           retry: false,
           finch: Canary.Finch
         ) do
      {:ok, %{status: status}} when status in 200..299 ->
        if track_circuit? do
          CircuitBreaker.record_success(webhook.id)

          WebhookDeliveries.mark_delivered(
            delivery_id,
            DateTime.utc_now() |> DateTime.to_iso8601()
          )
        end

        Logger.info("Webhook delivered to #{webhook.url}", event: event)
        :ok

      {:ok, %{status: status}} ->
        if track_circuit? do
          CircuitBreaker.record_failure(webhook.id)
          maybe_mark_final_discard(job, delivery_id, "http_#{status}")
        end

        Logger.warning("Webhook delivery failed: HTTP #{status}", event: event)
        {:error, "HTTP #{status}"}

      {:error, reason} ->
        if track_circuit? do
          CircuitBreaker.record_failure(webhook.id)
          maybe_mark_final_discard(job, delivery_id, "request_error")
        end

        Logger.warning("Webhook delivery error: #{inspect(reason)}", event: event)
        {:error, inspect(reason)}
    end
  end

  defp maybe_mark_final_discard(nil, _delivery_id, _reason), do: :ok

  defp maybe_mark_final_discard(%Oban.Job{} = job, delivery_id, reason) do
    attempt = job.attempt || 1
    max_attempts = job.max_attempts || __opts__()[:max_attempts] || 4

    if attempt >= max_attempts do
      WebhookDeliveries.mark_discarded(
        delivery_id,
        reason,
        DateTime.utc_now() |> DateTime.to_iso8601()
      )
    end
  end

  defp enqueue_delivery(webhook, payload, event, delivery_id, now) do
    args = %{webhook_id: webhook.id, payload: payload, event: event, delivery_id: delivery_id}

    try do
      Repo.transaction(fn ->
        WebhookDeliveries.create_pending(delivery_id, webhook.id, event, now)

        case args |> __MODULE__.new() |> Oban.insert() do
          {:ok, _job} -> :ok
          {:error, reason} -> Repo.rollback(reason)
        end
      end)
      |> case do
        {:ok, :ok} -> :ok
        {:error, reason} -> {:error, reason}
      end
    rescue
      error -> {:error, {:exception, error}}
    end
  end

  defp delivery_id_from_job(%Oban.Job{args: %{"delivery_id" => delivery_id}})
       when is_binary(delivery_id),
       do: delivery_id

  defp delivery_id_from_job(%Oban.Job{id: job_id}) when is_integer(job_id),
    do: "DLV-legacy-#{job_id}"

  defp delivery_id_from_job(%Oban.Job{args: args}), do: "DLV-legacy-#{stable_args_hash(args)}"

  defp cooldown_key(webhook_id, event, payload) do
    identity =
      case payload_identity(payload) do
        nil -> "payload"
        value -> value
      end

    "#{webhook_id}:#{event}:#{identity}"
  end

  defp payload_identity(payload) when is_map(payload) do
    cond do
      is_binary(get_in(payload, ["error", "group_hash"])) ->
        "error_group:#{get_in(payload, ["error", "group_hash"])}"

      is_binary(get_in(payload, ["incident", "id"])) ->
        "incident:#{get_in(payload, ["incident", "id"])}"

      is_map(payload["target"]) ->
        target = payload["target"]
        service = target["service"] || ""
        name = target["name"] || ""
        url = target["url"] || ""
        "target:#{service}:#{name}:#{url}"

      true ->
        stable_payload_hash(payload)
    end
  end

  defp payload_identity(_), do: nil

  defp stable_payload_hash(payload) do
    payload
    |> Map.drop(["timestamp", "sequence"])
    |> stable_hash()
    |> then(&"payload:#{&1}")
  end

  defp stable_args_hash(args) do
    args
    |> Map.drop(["delivery_id"])
    |> stable_hash()
    |> String.slice(0, 24)
  end

  defp stable_hash(value) do
    value
    |> canonicalize()
    |> :erlang.term_to_binary(minor_version: 1)
    |> then(&:crypto.hash(:sha256, &1))
    |> Base.encode16(case: :lower)
  end

  defp canonicalize(value) when is_map(value) do
    value
    |> Enum.map(fn {key, nested} -> {key, canonicalize(nested)} end)
    |> Enum.sort_by(fn {key, _nested} -> key end)
  end

  defp canonicalize(value) when is_list(value), do: Enum.map(value, &canonicalize/1)
  defp canonicalize(value), do: value
end
