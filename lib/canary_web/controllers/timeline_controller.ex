defmodule CanaryWeb.TimelineController do
  use CanaryWeb, :controller

  alias Canary.Timeline

  def index(conn, params) do
    case validate_service_param(Map.get(params, "service")) do
      :ok ->
        cursor = Map.get(params, "after") || Map.get(params, "cursor")

        opts = [
          service: Map.get(params, "service"),
          window: Map.get(params, "window", "24h"),
          limit: Map.get(params, "limit"),
          cursor: cursor,
          event_type: Map.get(params, "event_type")
        ]

        case Timeline.list(opts) do
          {:ok, timeline} ->
            json(conn, timeline)

          {:error, :invalid_window} ->
            CanaryWeb.Plugs.ProblemDetails.render_error(
              conn,
              422,
              "validation_error",
              "Invalid window. Allowed: 1h, 6h, 24h, 7d, 30d",
              %{errors: %{window: ["must be one of: 1h, 6h, 24h, 7d, 30d"]}}
            )

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

          {:error, {:invalid_event_type, invalid}} ->
            allowed = Canary.Webhooks.EventTypes.timeline() |> Enum.join(", ")

            CanaryWeb.Plugs.ProblemDetails.render_error(
              conn,
              422,
              "validation_error",
              "Invalid event_type: #{Enum.join(invalid, ", ")}. Allowed: #{allowed}",
              %{errors: %{event_type: ["must be one or more of: #{allowed}"]}}
            )
        end

      :error ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid service parameter. Must be a string.",
          %{errors: %{service: ["must be a string"]}}
        )
    end
  end

  defp validate_service_param(nil), do: :ok
  defp validate_service_param(service) when is_binary(service), do: :ok
  defp validate_service_param(_), do: :error
end
