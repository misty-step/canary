defmodule CanaryWeb.ServiceOnboardingController do
  use CanaryWeb, :controller

  alias Canary.ChangesetErrors
  alias Canary.ServiceOnboarding

  def create(conn, params) do
    case ServiceOnboarding.connect(params, base_url(conn)) do
      {:ok, payload} ->
        conn
        |> put_status(201)
        |> json(payload)

      {:error, {:validation, changeset}} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid service onboarding request.",
          %{errors: ChangesetErrors.format(changeset)}
        )

      {:error, :internal} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          500,
          "internal_error",
          "Failed to connect service."
        )
    end
  end

  defp base_url(conn) do
    host =
      case {conn.scheme, conn.port} do
        {:http, 80} -> conn.host
        {:https, 443} -> conn.host
        _ -> "#{conn.host}:#{conn.port}"
      end

    "#{conn.scheme}://#{host}"
  end
end
