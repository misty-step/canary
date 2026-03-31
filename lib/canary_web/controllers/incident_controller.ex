defmodule CanaryWeb.IncidentController do
  use CanaryWeb, :controller

  alias Canary.Query

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
    json(conn, %{incidents: incidents})
  end
end
