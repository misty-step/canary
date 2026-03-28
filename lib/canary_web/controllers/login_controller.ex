defmodule CanaryWeb.LoginController do
  use CanaryWeb, :controller

  alias Canary.Errors.RateLimiter

  def create(conn, %{"password" => password}) do
    case Application.get_env(:canary, :dashboard_password_hash) do
      nil ->
        redirect(conn, to: "/dashboard")

      hash ->
        ip = to_string(:inet.ntoa(conn.remote_ip))

        case RateLimiter.check(ip, :auth_fail) do
          {:error, retry_after} ->
            conn
            |> put_flash(:error, "Too many attempts. Try again in #{retry_after}s.")
            |> redirect(to: "/dashboard/login")

          :ok ->
            if Bcrypt.verify_pass(password, hash) do
              conn
              |> put_session("dashboard_auth_version", CanaryWeb.DashboardAuth.auth_version(hash))
              |> redirect(to: "/dashboard")
            else
              conn
              |> put_flash(:error, "Invalid password")
              |> redirect(to: "/dashboard/login")
            end
        end
    end
  end

  def create(conn, _params) do
    conn
    |> put_flash(:error, "Password required")
    |> redirect(to: "/dashboard/login")
  end
end
