defmodule Canary.WebhookDeliveries do
  @moduledoc """
  Durable read/write model for logical webhook deliveries.
  """

  import Ecto.Query

  alias Canary.{Repo, Repos}
  alias Canary.Schemas.WebhookDelivery

  @default_limit 50
  @max_limit 100

  @spec new_delivery_id() :: String.t()
  def new_delivery_id, do: Canary.ID.delivery_id()

  @spec delivery_id_for_job(Oban.Job.t()) :: String.t()
  def delivery_id_for_job(%Oban.Job{args: %{"delivery_id" => delivery_id}})
      when is_binary(delivery_id) and delivery_id != "" do
    delivery_id
  end

  def delivery_id_for_job(%Oban.Job{id: job_id}) when is_integer(job_id) do
    "DLV-job-#{job_id}"
  end

  def delivery_id_for_job(%Oban.Job{}), do: new_delivery_id()

  @spec ensure_queued(String.t(), String.t(), String.t()) :: :ok | {:error, term()}
  def ensure_queued(delivery_id, webhook_id, event) do
    case Repo.insert(
           delivery_changeset(delivery_id, %{
             webhook_id: webhook_id,
             event: event,
             status: "queued",
             attempt_count: 0,
             created_at: now()
           }),
           on_conflict: :nothing
         ) do
      {:ok, _delivery} -> :ok
      {:error, reason} -> {:error, reason}
    end
  end

  @spec record_attempt(String.t(), String.t(), String.t()) :: :ok | {:error, term()}
  def record_attempt(delivery_id, webhook_id, event) do
    with :ok <- ensure_queued(delivery_id, webhook_id, event) do
      update_delivery(delivery_id, fn delivery ->
        attempt_time = now()

        %{
          status: "retrying",
          attempt_count: delivery.attempt_count + 1,
          first_attempted_at: delivery.first_attempted_at || attempt_time,
          last_attempted_at: attempt_time,
          last_error: nil,
          suppression_reason: nil
        }
      end)
    end
  end

  @spec record_success(String.t(), integer()) :: :ok | {:error, term()}
  def record_success(delivery_id, status_code) do
    update_delivery(delivery_id, fn _delivery ->
      %{
        status: "delivered",
        last_status_code: status_code,
        last_error: nil,
        suppression_reason: nil,
        completed_at: now()
      }
    end)
  end

  @spec record_retry(String.t(), String.t(), integer() | nil) :: :ok | {:error, term()}
  def record_retry(delivery_id, reason, status_code) do
    update_delivery(delivery_id, fn _delivery ->
      %{
        status: "retrying",
        last_status_code: status_code,
        last_error: reason
      }
    end)
  end

  @spec record_discarded(String.t(), String.t(), integer() | nil) :: :ok | {:error, term()}
  def record_discarded(delivery_id, reason, status_code) do
    update_delivery(delivery_id, fn _delivery ->
      %{
        status: "discarded",
        last_status_code: status_code,
        last_error: reason,
        completed_at: now()
      }
    end)
  end

  @spec record_suppressed(String.t(), String.t(), String.t(), String.t()) ::
          :ok | {:error, term()}
  def record_suppressed(delivery_id, webhook_id, event, reason) do
    with :ok <- ensure_queued(delivery_id, webhook_id, event) do
      update_delivery(delivery_id, fn _delivery ->
        %{
          status: "suppressed",
          suppression_reason: reason,
          last_error: nil,
          completed_at: now()
        }
      end)
    end
  end

  @spec record_enqueue_failure(String.t(), String.t(), String.t(), String.t()) ::
          :ok | {:error, term()}
  def record_enqueue_failure(delivery_id, webhook_id, event, reason) do
    with :ok <- ensure_queued(delivery_id, webhook_id, event) do
      update_delivery(delivery_id, fn _delivery ->
        %{
          status: "discarded",
          last_error: reason,
          completed_at: now()
        }
      end)
    end
  end

  @spec list_for_webhook(String.t(), keyword()) :: [WebhookDelivery.t()]
  def list_for_webhook(webhook_id, opts \\ []) do
    limit = parse_limit(Keyword.get(opts, :limit))

    from(d in WebhookDelivery,
      where: d.webhook_id == ^webhook_id,
      order_by: [desc: d.created_at, desc: d.id],
      limit: ^limit
    )
    |> Repos.read_repo().all()
  end

  defp update_delivery(delivery_id, attrs_fun) do
    case Repo.get(WebhookDelivery, delivery_id) do
      nil ->
        {:error, :not_found}

      delivery ->
        case Repo.update(WebhookDelivery.changeset(delivery, attrs_fun.(delivery))) do
          {:ok, _delivery} -> :ok
          {:error, reason} -> {:error, reason}
        end
    end
  end

  defp delivery_changeset(delivery_id, attrs) do
    %WebhookDelivery{id: delivery_id}
    |> WebhookDelivery.changeset(attrs)
  end

  defp parse_limit(nil), do: @default_limit

  defp parse_limit(limit) when is_integer(limit) do
    limit
    |> max(1)
    |> min(@max_limit)
  end

  defp parse_limit(limit) when is_binary(limit) do
    case Integer.parse(limit) do
      {parsed, ""} -> parse_limit(parsed)
      _ -> @default_limit
    end
  end

  defp parse_limit(_), do: @default_limit

  defp now, do: DateTime.utc_now() |> DateTime.to_iso8601()
end
