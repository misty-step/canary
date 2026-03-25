defmodule CanaryWeb.ReportController do
  use CanaryWeb, :controller

  alias Canary.Report
  alias CanaryWeb.CsvView

  def index(conn, params) do
    opts = [
      window: Map.get(params, "window", "1h"),
      limit: Map.get(params, "limit"),
      cursor: Map.get(params, "cursor")
    ]

    case Report.generate(opts) do
      {:ok, report} -> render_report(conn, report)
      {:error, :invalid_cursor} -> render_invalid_cursor(conn)
      {:error, :invalid_limit} -> render_invalid_limit(conn)
      {:error, :invalid_window} -> render_invalid_window(conn)
    end
  end

  defp render_report(conn, report) do
    if get_format(conn) == "csv" do
      conn
      |> put_resp_content_type("text/csv")
      |> send_resp(200, CsvView.report(report))
    else
      json(conn, report)
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

  defp render_invalid_limit(conn) do
    CanaryWeb.Plugs.ProblemDetails.render_error(
      conn,
      422,
      "validation_error",
      "Invalid limit. Expected a positive integer.",
      %{errors: %{limit: ["must be a positive integer"]}}
    )
  end

  defp render_invalid_cursor(conn) do
    CanaryWeb.Plugs.ProblemDetails.render_error(
      conn,
      422,
      "validation_error",
      "Invalid cursor.",
      %{errors: %{cursor: ["must be a valid pagination cursor"]}}
    )
  end
end
