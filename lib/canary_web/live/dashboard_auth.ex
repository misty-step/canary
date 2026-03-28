defmodule CanaryWeb.DashboardAuth do
  @moduledoc "LiveView on_mount hook that gates dashboard access behind DASHBOARD_PASSWORD."

  import Phoenix.LiveView

  def on_mount(:default, _params, session, socket) do
    if auth_disabled?() or session["dashboard_authenticated"] do
      {:cont, socket}
    else
      {:halt, redirect(socket, to: "/dashboard/login")}
    end
  end

  defp auth_disabled?, do: is_nil(Application.get_env(:canary, :dashboard_password_hash))
end
