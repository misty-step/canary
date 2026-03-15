defmodule CanaryWeb.KeyController do
  use CanaryWeb, :controller

  alias Canary.Auth

  def index(conn, _params) do
    keys = Auth.list_keys()

    json(conn, %{
      keys:
        Enum.map(keys, fn k ->
          %{
            id: k.id,
            name: k.name,
            key_prefix: k.key_prefix,
            active: Canary.Schemas.ApiKey.active?(k),
            created_at: k.created_at,
            revoked_at: k.revoked_at
          }
        end)
    })
  end

  def create(conn, params) do
    name = params["name"] || "unnamed"

    case Auth.generate_key(name) do
      {:ok, key, raw_key} ->
        conn
        |> put_status(201)
        |> json(%{
          id: key.id,
          name: key.name,
          key: raw_key,
          key_prefix: key.key_prefix,
          created_at: key.created_at,
          warning: "Store this key securely. It will not be shown again."
        })

      {:error, _} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          500,
          "internal_error",
          "Failed to generate API key."
        )
    end
  end

  def revoke(conn, %{"id" => id}) do
    case Auth.revoke_key(id) do
      {:ok, _} ->
        json(conn, %{status: "revoked"})

      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "API key not found."
        )
    end
  end
end
