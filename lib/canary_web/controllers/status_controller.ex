defmodule CanaryWeb.StatusController do
  use CanaryWeb, :controller

  def index(conn, params) do
    window = Map.get(params, "window", "1h")

    case Canary.Status.combined(window) do
      {:ok, status} ->
        json(conn, status)

      {:error, :invalid_window} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid window. Allowed: 1h, 6h, 24h, 7d, 30d",
          %{errors: %{window: ["must be one of: 1h, 6h, 24h, 7d, 30d"]}}
        )
    end
  end
end
