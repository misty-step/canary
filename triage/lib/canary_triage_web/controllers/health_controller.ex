defmodule CanaryTriageWeb.HealthController do
  use CanaryTriageWeb, :controller

  def healthz(conn, _params) do
    json(conn, %{status: "ok"})
  end
end
