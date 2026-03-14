defmodule Canary.Workers.WebhookDelivery do
  @moduledoc """
  Oban worker for webhook delivery with retries, HMAC signing,
  circuit breaker checks, and cooldown enforcement.
  """

  use Oban.Worker,
    queue: :webhooks,
    max_attempts: 4,
    priority: 1

  alias Canary.ReadRepo
  alias Canary.Schemas.Webhook
  alias Canary.Alerter.{Signer, CircuitBreaker, Cooldown}
  import Ecto.Query

  require Logger

  @delivery_timeout 10_000

  @impl Oban.Worker
  def perform(%Oban.Job{args: %{"webhook_id" => webhook_id, "payload" => payload, "event" => event}}) do
    case ReadRepo.get(Webhook, webhook_id) do
      nil ->
        Logger.warning("Webhook #{webhook_id} not found, discarding")
        :ok

      %Webhook{active: 0} ->
        Logger.info("Webhook #{webhook_id} inactive, skipping")
        :ok

      webhook ->
        deliver(webhook, payload, event)
    end
  end

  def deliver(webhook, payload, event) do
    if CircuitBreaker.open?(webhook.id) and not CircuitBreaker.should_probe?(webhook.id) do
      Logger.info("Circuit open for #{webhook.id}, skipping")
      :ok
    else
      body = Jason.encode!(payload)
      delivery_id = Canary.ID.generate()

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
          CircuitBreaker.record_success(webhook.id)
          Logger.info("Webhook delivered to #{webhook.url}", event: event)
          :ok

        {:ok, %{status: status}} ->
          CircuitBreaker.record_failure(webhook.id)
          Logger.warning("Webhook delivery failed: HTTP #{status}", event: event)
          {:error, "HTTP #{status}"}

        {:error, reason} ->
          CircuitBreaker.record_failure(webhook.id)
          Logger.warning("Webhook delivery error: #{inspect(reason)}", event: event)
          {:error, inspect(reason)}
      end
    end
  end

  @spec enqueue_for_event(String.t(), map()) :: :ok
  def enqueue_for_event(event, payload) do
    webhooks =
      from(w in Webhook, where: w.active == 1)
      |> ReadRepo.all()
      |> Enum.filter(&Webhook.subscribes_to?(&1, event))

    Enum.each(webhooks, fn webhook ->
      unless Cooldown.in_cooldown?("#{webhook.id}:#{event}") do
        Cooldown.mark("#{webhook.id}:#{event}")

        %{webhook_id: webhook.id, payload: payload, event: event}
        |> __MODULE__.new()
        |> Oban.insert()
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
end
