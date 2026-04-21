defmodule CanaryWeb.IncidentController do
  use CanaryWeb, :controller

  alias Canary.{Query, Summary}

  def index(conn, params) do
    opts =
      Enum.reject(
        [
          with_annotation: params["with_annotation"],
          without_annotation: params["without_annotation"]
        ],
        fn {_, v} -> is_nil(v) or v == "" end
      )

    incidents = Query.active_incidents(opts)
    json(conn, %{summary: Summary.incidents_list(incidents), incidents: incidents})
  end

  def show(conn, %{"id" => id}) do
    case Query.incident_detail(id) do
      {:ok, detail} ->
        json(conn, detail)

      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "Incident #{id} not found."
        )
    end
  end
end
