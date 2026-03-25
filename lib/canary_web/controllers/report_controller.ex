defmodule CanaryWeb.ReportController do
  use CanaryWeb, :controller

  alias Canary.Report

  def index(conn, params) do
    window = Map.get(params, "window", "1h")
    q = Map.get(params, "q")

    case Report.generate(window: window, q: q) do
      {:ok, report} -> json(conn, report)
      {:error, :invalid_window} -> render_invalid_window(conn)
      {:error, :invalid_query} -> render_invalid_query(conn)
    end
  end

  defp render_invalid_window(conn) do
    CanaryWeb.Plugs.ProblemDetails.render_error(
      conn,
      422,
      "validation_error",
      "Invalid window. Allowed: 1h, 6h, 24h, 7d, 30d",
      %{errors: %{window: ["must be one of: 1h, 6h, 24h, 7d, 30d"]}}
    )
  end

  defp render_invalid_query(conn) do
    CanaryWeb.Plugs.ProblemDetails.render_error(
      conn,
      422,
      "validation_error",
      "Invalid q parameter. Must be a string.",
      %{errors: %{q: ["must be a string"]}}
    )
  end
end
