defmodule CanaryWeb.DashboardAuth do
  @moduledoc "LiveView on_mount hook that gates dashboard access behind DASHBOARD_PASSWORD."

  import Phoenix.LiveView

  def on_mount(:default, _params, session, socket) do
    if authenticated?(session) do
      {:cont, socket}
    else
      {:halt, redirect(socket, to: "/dashboard/login")}
    end
  end

  @doc "Returns true when no password is configured or the session holds a valid auth version."
  def authenticated?(session) do
    auth_disabled?() or valid_session?(session)
  end

  defp auth_disabled?, do: is_nil(Application.get_env(:canary, :dashboard_password_hash))

  def current_version, do: Application.get_env(:canary, :dashboard_auth_version)

  defp valid_session?(session) do
    with version when is_binary(version) <- session["dashboard_auth_version"],
         current when is_binary(current) <- current_version() do
      version == current
    else
      _ -> false
    end
  end
end
