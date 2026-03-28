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

  defp valid_session?(session) do
    with version when is_binary(version) <- session["dashboard_auth_version"],
         hash when is_binary(hash) <- Application.get_env(:canary, :dashboard_password_hash) do
      version == CanaryWeb.LoginController.auth_version(hash)
    else
      _ -> false
    end
  end
end
