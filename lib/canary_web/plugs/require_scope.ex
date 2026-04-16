defmodule CanaryWeb.Plugs.RequireScope do
  @moduledoc "Enforces API key authorization scopes at the router boundary."

  alias Canary.Schemas.ApiKey

  @type permission :: :admin | :ingest | :read

  def init(permission) when permission in [:admin, :ingest, :read], do: permission

  def call(%Plug.Conn{assigns: %{api_key: %ApiKey{} = api_key}} = conn, permission) do
    if ApiKey.allows?(api_key, permission) do
      conn
    else
      CanaryWeb.Plugs.ProblemDetails.render_error(
        conn,
        403,
        "insufficient_scope",
        detail(api_key.scope, permission),
        %{
          scope: api_key.scope,
          required_scopes: ApiKey.allowed_scopes(permission)
        }
      )
    end
  end

  def call(conn, _permission), do: conn

  defp detail(scope, permission) do
    allowed =
      permission
      |> ApiKey.allowed_scopes()
      |> Enum.map_join(" or ", &"`#{&1}`")

    "API key scope `#{scope}` cannot access this #{permission_label(permission)} endpoint. Use an #{allowed} key."
  end

  defp permission_label(:admin), do: "admin"
  defp permission_label(:ingest), do: "ingest"
  defp permission_label(:read), do: "read"
end
