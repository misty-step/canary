defmodule CanaryWeb.LoginController do
  use CanaryWeb, :controller

  alias Canary.Errors.RateLimiter

  def create(conn, %{"password" => password}) do
    case Application.get_env(:canary, :dashboard_password_hash) do
      nil ->
        redirect(conn, to: "/dashboard")

      hash ->
        ip = to_string(:inet.ntoa(conn.remote_ip))

        if Bcrypt.verify_pass(password, hash) do
          conn
          |> put_session("dashboard_auth_version", auth_version(hash))
          |> redirect(to: "/dashboard")
        else
          RateLimiter.check(ip, :auth_fail)

          conn
          |> put_flash(:error, "Invalid password")
          |> redirect(to: "/dashboard/login")
        end
    end
  end

  def create(conn, _params) do
    conn
    |> put_flash(:error, "Password required")
    |> redirect(to: "/dashboard/login")
  end

  @doc false
  def auth_version(hash), do: :crypto.hash(:sha256, hash) |> Base.url_encode64(padding: false)
end
