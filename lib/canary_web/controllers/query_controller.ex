defmodule CanaryWeb.QueryController do
  use CanaryWeb, :controller

  alias Canary.Query

  def query(conn, %{"service" => service} = params) do
    window = params["window"] || "1h"
    cursor = params["cursor"]

    case Query.errors_by_service(service, window, cursor) do
      {:ok, result} -> json(conn, result)
      {:error, :invalid_window} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn, 422, "validation_error",
          "Invalid window. Allowed: 1h, 6h, 24h, 7d, 30d",
          %{errors: %{window: ["must be one of: 1h, 6h, 24h, 7d, 30d"]}}
        )
    end
  end

  def query(conn, %{"group_by" => "error_class"} = params) do
    window = params["window"] || "24h"

    case Query.errors_by_class(window) do
      {:ok, result} -> json(conn, result)
      {:error, :invalid_window} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn, 422, "validation_error",
          "Invalid window.",
          %{errors: %{window: ["must be one of: 1h, 6h, 24h, 7d, 30d"]}}
        )
    end
  end

  def query(conn, _params) do
    CanaryWeb.Plugs.ProblemDetails.render_error(
      conn, 422, "validation_error",
      "Provide 'service' or 'group_by=error_class' parameter."
    )
  end

  def show(conn, %{"id" => id}) do
    case Query.error_detail(id) do
      {:ok, result} -> json(conn, result)
      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn, 404, "not_found", "Error #{id} not found."
        )
    end
  end
end
