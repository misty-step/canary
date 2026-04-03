defmodule CanaryWeb.WebhookDeliveryController do
  use CanaryWeb, :controller

  alias Canary.WebhookDeliveries

  def index(conn, params) do
    cursor = Map.get(params, "after") || Map.get(params, "cursor")

    with :ok <- validate_string_param(:webhook_id, Map.get(params, "webhook_id")),
         :ok <- validate_string_param(:event, Map.get(params, "event")),
         :ok <- validate_status(Map.get(params, "status")),
         {:ok, page} <-
           WebhookDeliveries.page(
             webhook_id: Map.get(params, "webhook_id"),
             event: Map.get(params, "event"),
             status: Map.get(params, "status"),
             limit: Map.get(params, "limit"),
             cursor: cursor
           ) do
      json(conn, %{
        returned_count: page.returned_count,
        cursor: page.cursor,
        deliveries: Enum.map(page.deliveries, &format_delivery/1)
      })
    else
      {:error, :invalid_limit} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid limit. Expected a positive integer up to 200.",
          %{errors: %{limit: ["must be a positive integer no greater than 200"]}}
        )

      {:error, :invalid_cursor} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid cursor.",
          %{errors: %{cursor: ["must be a valid pagination cursor"]}}
        )

      {:error, {:invalid_param, param}} ->
        param = to_string(param)

        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid #{param} parameter. Must be a string.",
          %{errors: %{param => ["must be a string"]}}
        )

      {:error, :invalid_status} ->
        allowed = WebhookDeliveries.statuses() |> Enum.join(", ")

        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid status. Allowed: #{allowed}",
          %{errors: %{status: ["must be one of: #{allowed}"]}}
        )
    end
  end

  defp validate_string_param(_name, nil), do: :ok
  defp validate_string_param(_name, value) when is_binary(value), do: :ok
  defp validate_string_param(name, _value), do: {:error, {:invalid_param, name}}

  defp validate_status(nil), do: :ok

  defp validate_status(status) when is_binary(status) do
    if status in WebhookDeliveries.statuses(), do: :ok, else: {:error, :invalid_status}
  end

  defp validate_status(_status), do: {:error, {:invalid_param, :status}}

  defp format_delivery(delivery) do
    %{
      delivery_id: delivery.delivery_id,
      webhook_id: delivery.webhook_id,
      event: delivery.event,
      status: delivery.status,
      attempt_count: delivery.attempt_count,
      reason: delivery.reason,
      first_attempt_at: delivery.first_attempt_at,
      last_attempt_at: delivery.last_attempt_at,
      delivered_at: delivery.delivered_at,
      discarded_at: delivery.discarded_at,
      completed_at: completed_at(delivery),
      created_at: delivery.created_at,
      updated_at: delivery.updated_at
    }
  end

  defp completed_at(%{delivered_at: value}) when is_binary(value), do: value
  defp completed_at(%{discarded_at: value}) when is_binary(value), do: value

  defp completed_at(%{status: status, updated_at: updated_at})
       when status in ["suppressed", "discarded", "delivered"] do
    updated_at
  end

  defp completed_at(_), do: nil
end
