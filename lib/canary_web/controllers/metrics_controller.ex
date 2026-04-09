defmodule CanaryWeb.MetricsController do
  use CanaryWeb, :controller

  def index(conn, _params) do
    conn
    |> register_before_send(fn conn ->
      if conn.status in 200..299 do
        put_resp_header(conn, "content-type", "text/plain; version=0.0.4; charset=utf-8")
      else
        conn
      end
    end)
    |> text(Canary.Metrics.scrape())
  end
end
