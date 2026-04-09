defmodule Canary.WebhookDeliveries do
  @moduledoc """
  Persistence API for outbound webhook delivery attempts and outcomes.
  """

  import Ecto.Query

  alias Canary.Repo
  alias Canary.Schemas.WebhookDeliveryLedger

  require Logger

  @default_limit 50
  @max_limit 200
  @statuses ~w(pending retrying delivered discarded suppressed)

  @spec statuses() :: [String.t()]
  def statuses, do: @statuses

  @spec create_pending(String.t(), String.t(), String.t(), String.t()) :: :ok
  def create_pending(delivery_id, webhook_id, event, now) do
    attrs = %{
      webhook_id: webhook_id,
      event: event,
      status: "pending",
      attempt_count: 0,
      created_at: now,
      updated_at: now
    }

    %WebhookDeliveryLedger{delivery_id: delivery_id}
    |> WebhookDeliveryLedger.changeset(attrs)
    |> Repo.insert(on_conflict: :nothing, conflict_target: :delivery_id)
    |> log_result("create pending webhook delivery #{delivery_id}")
  end

  @spec create_suppressed(String.t(), String.t(), String.t(), String.t(), String.t()) :: :ok
  def create_suppressed(delivery_id, webhook_id, event, reason, now) do
    attrs = %{
      webhook_id: webhook_id,
      event: event,
      status: "suppressed",
      attempt_count: 0,
      reason: reason,
      created_at: now,
      updated_at: now
    }

    case Repo.insert(
           WebhookDeliveryLedger.changeset(
             %WebhookDeliveryLedger{delivery_id: delivery_id},
             attrs
           ),
           on_conflict: [set: [status: "suppressed", reason: reason, updated_at: now]],
           conflict_target: :delivery_id
         ) do
      {:ok, _row} ->
        emit_delivery_metric("suppressed")
        :ok

      {:error, changeset} ->
        log_result({:error, changeset}, "mark webhook delivery #{delivery_id} as suppressed")
    end
  end

  @spec mark_attempt(String.t(), String.t()) :: :ok
  def mark_attempt(delivery_id, now) do
    from(d in WebhookDeliveryLedger,
      where: d.delivery_id == ^delivery_id,
      update: [
        set: [
          status:
            fragment(
              "CASE WHEN status IN ('pending', 'retrying') THEN 'retrying' ELSE status END"
            ),
          attempt_count: fragment("attempt_count + 1"),
          first_attempt_at: fragment("COALESCE(first_attempt_at, ?)", ^now),
          last_attempt_at: ^now,
          updated_at: ^now
        ]
      ]
    )
    |> Repo.update_all([])

    :ok
  end

  @spec mark_delivered(String.t(), String.t()) :: :ok
  def mark_delivered(delivery_id, now) do
    case Repo.get(WebhookDeliveryLedger, delivery_id) do
      nil ->
        :ok

      row ->
        persist_update(
          row,
          %{status: "delivered", delivered_at: now, updated_at: now},
          "delivered"
        )
    end
  end

  @spec mark_discarded(String.t(), String.t(), String.t()) :: :ok
  def mark_discarded(delivery_id, reason, now) do
    case Repo.get(WebhookDeliveryLedger, delivery_id) do
      nil ->
        :ok

      row ->
        persist_update(
          row,
          %{
            status: "discarded",
            reason: reason,
            discarded_at: now,
            updated_at: now
          },
          "discarded"
        )
    end
  end

  @spec list(keyword()) :: [WebhookDeliveryLedger.t()]
  def list(opts \\ []) do
    limit = Keyword.get(opts, :limit, @default_limit)

    opts
    |> base_query()
    |> limit(^limit)
    |> Canary.Repos.read_repo().all()
  end

  @spec page(keyword()) ::
          {:ok,
           %{
             deliveries: [WebhookDeliveryLedger.t()],
             returned_count: non_neg_integer(),
             cursor: String.t() | nil
           }}
          | {:error, :invalid_limit | :invalid_cursor}
  def page(opts \\ []) do
    with {:ok, limit} <- parse_limit(Keyword.get(opts, :limit)),
         {:ok, cursor} <- decode_cursor(Keyword.get(opts, :cursor)) do
      rows =
        opts
        |> base_query()
        |> maybe_apply_cursor(cursor)
        |> limit(^(limit + 1))
        |> Canary.Repos.read_repo().all()

      {deliveries, next_cursor} = paginate(rows, limit)

      {:ok,
       %{
         deliveries: deliveries,
         returned_count: length(deliveries),
         cursor: next_cursor
       }}
    end
  end

  defp base_query(opts) do
    from(d in WebhookDeliveryLedger, order_by: [desc: d.created_at, desc: d.delivery_id])
    |> maybe_filter_delivery_id(Keyword.get(opts, :delivery_id))
    |> maybe_filter_webhook_id(Keyword.get(opts, :webhook_id))
    |> maybe_filter_event(Keyword.get(opts, :event))
    |> maybe_filter_status(Keyword.get(opts, :status))
  end

  defp maybe_filter_delivery_id(query, nil), do: query

  defp maybe_filter_delivery_id(query, delivery_id),
    do: from(d in query, where: d.delivery_id == ^delivery_id)

  defp maybe_filter_webhook_id(query, nil), do: query

  defp maybe_filter_webhook_id(query, webhook_id),
    do: from(d in query, where: d.webhook_id == ^webhook_id)

  defp maybe_filter_event(query, nil), do: query
  defp maybe_filter_event(query, event), do: from(d in query, where: d.event == ^event)

  defp maybe_filter_status(query, nil), do: query
  defp maybe_filter_status(query, status), do: from(d in query, where: d.status == ^status)

  defp parse_limit(nil), do: {:ok, @default_limit}

  defp parse_limit(limit) when is_integer(limit) and limit > 0 and limit <= @max_limit,
    do: {:ok, limit}

  defp parse_limit(limit) when is_binary(limit) do
    case Integer.parse(limit) do
      {value, ""} when value > 0 and value <= @max_limit -> {:ok, value}
      _ -> {:error, :invalid_limit}
    end
  end

  defp parse_limit(_), do: {:error, :invalid_limit}

  defp decode_cursor(nil), do: {:ok, nil}
  defp decode_cursor(""), do: {:ok, nil}

  defp decode_cursor(cursor) when is_binary(cursor) do
    with {:ok, decoded} <- Base.url_decode64(cursor, padding: false),
         {:ok, %{"created_at" => created_at, "delivery_id" => delivery_id}} <-
           Jason.decode(decoded),
         true <- is_binary(created_at) and is_binary(delivery_id) do
      {:ok, %{created_at: created_at, delivery_id: delivery_id}}
    else
      _ -> {:error, :invalid_cursor}
    end
  end

  defp decode_cursor(_), do: {:error, :invalid_cursor}

  defp maybe_apply_cursor(query, nil), do: query

  defp maybe_apply_cursor(query, %{created_at: created_at, delivery_id: delivery_id}) do
    from(d in query,
      where:
        d.created_at < ^created_at or
          (d.created_at == ^created_at and d.delivery_id < ^delivery_id)
    )
  end

  defp paginate(rows, limit) do
    {page, rest} = Enum.split(rows, limit)

    next_cursor =
      case {rest, List.last(page)} do
        {[], _} -> nil
        {_, nil} -> nil
        {_, last} -> encode_cursor(last)
      end

    {page, next_cursor}
  end

  defp encode_cursor(row) do
    %{created_at: row.created_at, delivery_id: row.delivery_id}
    |> Jason.encode!()
    |> Base.url_encode64(padding: false)
  end

  defp persist_update(row, attrs, status) do
    case Repo.update(WebhookDeliveryLedger.changeset(row, attrs)) do
      {:ok, _updated_row} ->
        emit_delivery_metric(status)
        :ok

      {:error, changeset} ->
        log_result({:error, changeset}, "update webhook delivery #{row.delivery_id}")
    end
  end

  defp log_result({:ok, _row}, _action), do: :ok

  defp log_result({:error, changeset}, action) do
    Logger.error("Failed to #{action}: #{inspect(changeset.errors)}")
    :ok
  end

  defp emit_delivery_metric(status) do
    :telemetry.execute(
      [:canary, :webhook, :delivery],
      %{count: 1},
      %{status: status}
    )
  end
end
