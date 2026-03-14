defmodule CanaryWeb.HealthController do
  use CanaryWeb, :controller

  alias Canary.Query

  def status(conn, _params) do
    json(conn, Query.health_status())
  end

  def target_checks(conn, %{"id" => id} = params) do
    window = params["window"] || "24h"

    case Query.target_checks(id, window) do
      {:ok, checks} ->
        json(conn, %{
          target_id: id,
          window: window,
          checks:
            Enum.map(checks, fn c ->
              %{
                checked_at: c.checked_at,
                result: c.result,
                status_code: c.status_code,
                latency_ms: c.latency_ms,
                tls_expires_at: c.tls_expires_at,
                error_detail: c.error_detail
              }
            end)
        })

      {:error, :invalid_window} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn, 422, "validation_error", "Invalid window."
        )
    end
  end

  def healthz(conn, _params) do
    conn |> put_status(200) |> json(%{status: "ok"})
  end

  def readyz(conn, _params) do
    checks = %{
      database: check_database(),
      supervisor: check_supervisor()
    }

    all_ok = Enum.all?(checks, fn {_k, v} -> v == :ok end)

    status = if all_ok, do: 200, else: 503

    conn
    |> put_status(status)
    |> json(%{
      status: if(all_ok, do: "ready", else: "not_ready"),
      checks: Map.new(checks, fn {k, v} -> {k, to_string(v)} end)
    })
  end

  defp check_database do
    Canary.Repo.query!("SELECT 1")
    :ok
  rescue
    _ -> :error
  end

  defp check_supervisor do
    case Process.whereis(Canary.Health.Supervisor) do
      nil -> :error
      pid when is_pid(pid) -> :ok
    end
  end
end
