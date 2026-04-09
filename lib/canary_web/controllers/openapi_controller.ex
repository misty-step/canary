defmodule CanaryWeb.OpenAPIController do
  use CanaryWeb, :controller

  @external_resource "priv/openapi/openapi.json"
  @openapi_document File.read!("priv/openapi/openapi.json")

  def index(conn, _params) do
    conn
    |> put_resp_content_type("application/json")
    |> send_resp(200, @openapi_document)
  end
end
