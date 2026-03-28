defmodule CanaryWeb.LoginController do
  use CanaryWeb, :controller

  def create(conn, %{"password" => password}) do
    case Application.get_env(:canary, :dashboard_password_hash) do
      nil ->
        redirect(conn, to: "/dashboard")

      hash ->
        if Bcrypt.verify_pass(password, hash) do
          conn
          |> put_session("dashboard_authenticated", true)
          |> redirect(to: "/dashboard")
        else
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
end
