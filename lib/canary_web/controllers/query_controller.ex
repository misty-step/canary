defmodule CanaryWeb.QueryController do
  use CanaryWeb, :controller

  alias Canary.Query

  def query(conn, %{"error_class" => error_class} = params) do
    # Cross-service queries default to wider window than per-service queries (24h vs 1h)
    window = params["window"] || "24h"

    opts =
      [service: params["service"], cursor: params["cursor"]]
      |> Enum.reject(fn {_, v} -> is_nil(v) end)
      |> Kernel.++(annotation_opts(params))

    case Query.errors_by_error_class(error_class, window, opts) do
      {:ok, result} -> json(conn, result)
      {:error, :invalid_window} -> render_invalid_window(conn)
    end
  end

  def query(conn, %{"service" => service} = params) do
    window = params["window"] || "1h"

    opts =
      [{:cursor, params["cursor"]} | annotation_opts(params)]
      |> Enum.reject(fn {_, v} -> is_nil(v) end)

    case Query.errors_by_service(service, window, opts) do
      {:ok, result} -> json(conn, result)
      {:error, :invalid_window} -> render_invalid_window(conn)
    end
  end

  def query(conn, %{"group_by" => "error_class"} = params) do
    window = params["window"] || "24h"

    case Query.errors_by_class(window) do
      {:ok, result} -> json(conn, result)
      {:error, :invalid_window} -> render_invalid_window(conn)
    end
  end

  def query(conn, _params) do
    CanaryWeb.Plugs.ProblemDetails.render_error(
      conn,
      422,
      "validation_error",
      "Provide 'service', 'error_class', or 'group_by=error_class' parameter."
    )
  end

  def show(conn, %{"id" => id}) do
    case Query.error_detail(id) do
      {:ok, result} ->
        json(conn, result)

      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "Error #{id} not found."
        )
    end
  end

  defp annotation_opts(params) do
    Enum.reject(
      [
        with_annotation: params["with_annotation"],
        without_annotation: params["without_annotation"]
      ],
      fn {_, v} -> is_nil(v) end
    )
  end

  defp render_invalid_window(conn) do
    CanaryWeb.Plugs.ProblemDetails.render_error(
      conn,
      422,
      "validation_error",
      "Invalid window. Allowed: 1h, 6h, 24h, 7d, 30d",
      %{errors: %{window: ["must be one of: 1h, 6h, 24h, 7d, 30d"]}}
    )
  end
end
