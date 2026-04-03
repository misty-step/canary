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
  def perform(
        %Oban.Job{
          args: %{"webhook_id" => webhook_id, "payload" => payload, "event" => event}
        } = job
      ) do
    delivery_id = WebhookDeliveries.delivery_id_for_job(job)
    record_ledger(fn -> WebhookDeliveries.ensure_queued(delivery_id, webhook_id, event) end)

    case Canary.Repos.read_repo().get(Webhook, webhook_id) do
      nil ->
        Logger.warning("Webhook #{webhook_id} not found, discarding")

        record_ledger(fn ->
          WebhookDeliveries.record_discarded(delivery_id, "Webhook not found", nil)
        end)

        :ok

      %Webhook{active: 0} ->
        Logger.info("Webhook #{webhook_id} inactive, skipping")

        record_ledger(fn ->
          WebhookDeliveries.record_suppressed(delivery_id, webhook_id, event, "webhook_inactive")
        end)

        :ok

      webhook ->
        deliver(webhook, payload, event, delivery_id, job)
    end
  end

  def deliver_test(webhook, payload, event) do
    case send_request(webhook, payload, event, false, WebhookDeliveries.new_delivery_id()) do
      {:ok, _status} -> :ok
      {:error, reason, _status} -> {:error, reason}
    end
  end

  def deliver(webhook, payload, event, delivery_id, job) do
    if CircuitBreaker.open?(webhook.id) and not CircuitBreaker.should_probe?(webhook.id) do
      Logger.info("Circuit open for #{webhook.id}, skipping")

      record_ledger(fn ->
        WebhookDeliveries.record_suppressed(delivery_id, webhook.id, event, "circuit_open")
      end)

      :ok
    else
      record_ledger(fn -> WebhookDeliveries.record_attempt(delivery_id, webhook.id, event) end)

      case send_request(webhook, payload, event, true, delivery_id) do
        {:ok, status} ->
          record_ledger(fn -> WebhookDeliveries.record_success(delivery_id, status) end)
          :ok

        {:error, reason, status} ->
          if final_attempt?(job) do
            record_ledger(fn ->
              WebhookDeliveries.record_discarded(delivery_id, reason, status)
            end)
          else
            record_ledger(fn -> WebhookDeliveries.record_retry(delivery_id, reason, status) end)
          end

          {:error, reason}
      end
    end
  end

  @spec enqueue_for_event(String.t(), map()) :: :ok
  def enqueue_for_event(event, payload) do
    webhooks =
      from(w in Webhook, where: w.active == 1)
      |> Canary.Repos.read_repo().all()
      |> Enum.filter(&Webhook.subscribes_to?(&1, event))

    Enum.each(webhooks, fn webhook ->
      delivery_id = WebhookDeliveries.new_delivery_id()
      cooldown_key = "#{webhook.id}:#{event}"

      unless Cooldown.in_cooldown?(cooldown_key) do
        case enqueue_delivery(webhook, payload, event, delivery_id) do
          :ok ->
            Cooldown.mark(cooldown_key)

          {:error, reason} ->
            Logger.error("Failed to enqueue webhook delivery",
              webhook_id: webhook.id,
              event: event
            )

            record_ledger(fn ->
              WebhookDeliveries.record_enqueue_failure(
                delivery_id,
                webhook.id,
                event,
                "enqueue_failed: #{inspect(reason)}"
              )
            end)
        end
      else
        record_ledger(fn ->
          WebhookDeliveries.record_suppressed(delivery_id, webhook.id, event, "cooldown")
        end)
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

  defp send_request(webhook, payload, event, track_circuit?, delivery_id) do
    body = Jason.encode!(payload)

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
        if track_circuit?, do: CircuitBreaker.record_success(webhook.id)
        Logger.info("Webhook delivered to #{webhook.url}", event: event)
        {:ok, status}

      {:ok, %{status: status}} ->
        if track_circuit?, do: CircuitBreaker.record_failure(webhook.id)
        Logger.warning("Webhook delivery failed: HTTP #{status}", event: event)
        {:error, "HTTP #{status}", status}

      {:error, reason} ->
        if track_circuit?, do: CircuitBreaker.record_failure(webhook.id)
        Logger.warning("Webhook delivery error: #{inspect(reason)}", event: event)
        {:error, inspect(reason), nil}
    end
  end

  defp final_attempt?(%Oban.Job{} = job) do
    attempt = job.attempt || 1
    max_attempts = job.max_attempts || __MODULE__.__opts__()[:max_attempts] || 1
    attempt >= max_attempts
  end

  defp enqueue_delivery(webhook, payload, event, delivery_id) do
    args = %{
      webhook_id: webhook.id,
      payload: payload,
      event: event,
      delivery_id: delivery_id
    }

    try do
      Repo.transaction(fn ->
        with :ok <- WebhookDeliveries.ensure_queued(delivery_id, webhook.id, event),
             {:ok, _job} <- args |> __MODULE__.new() |> Oban.insert() do
          :ok
        else
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

  defp record_ledger(fun) do
    case fun.() do
      :ok ->
        :ok

      {:error, reason} ->
        Logger.warning("Failed to record webhook delivery ledger state", reason: inspect(reason))
        :ok
    end
  end
end
