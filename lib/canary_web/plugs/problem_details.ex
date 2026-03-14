defmodule CanaryWeb.Plugs.ProblemDetails do
  @moduledoc """
  RFC 9457 Problem Details error responses.
  Content-Type: application/problem+json
  """

  import Plug.Conn
  import Phoenix.Controller, only: [json: 2]

  @base_url "https://canary.dev/problems"

  def render_error(conn, status, code, detail, extra \\ %{}) do
    title = title_for(code)

    body =
      %{
        type: "#{@base_url}/#{String.replace(code, "_", "-")}",
        title: title,
        status: status,
        detail: detail,
        code: code,
        request_id: conn.assigns[:request_id] || Logger.metadata()[:request_id]
      }
      |> Map.merge(extra)

    conn
    |> put_resp_content_type("application/problem+json")
    |> put_status(status)
    |> json(body)
    |> halt()
  end

  defp title_for("invalid_request"), do: "Invalid Request"
  defp title_for("invalid_api_key"), do: "Invalid API Key"
  defp title_for("not_found"), do: "Not Found"
  defp title_for("payload_too_large"), do: "Payload Too Large"
  defp title_for("validation_error"), do: "Validation Error"
  defp title_for("rate_limited"), do: "Rate Limit Exceeded"
  defp title_for("internal_error"), do: "Internal Server Error"
  defp title_for("unavailable"), do: "Service Unavailable"
  defp title_for(code), do: code |> String.replace("_", " ") |> String.capitalize()
end
