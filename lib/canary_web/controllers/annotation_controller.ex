defmodule CanaryWeb.AnnotationController do
  use CanaryWeb, :controller

  alias Canary.Annotations

  def create(conn, %{"incident_id" => incident_id} = params) do
    do_create(
      conn,
      &Annotations.create_for_incident(incident_id, &1),
      "Incident not found.",
      params
    )
  end

  def index(conn, %{"incident_id" => incident_id}) do
    annotations = Annotations.list_for_incident(incident_id)
    json(conn, %{annotations: Enum.map(annotations, &Annotations.format/1)})
  end

  def group_create(conn, %{"group_hash" => group_hash} = params) do
    do_create(
      conn,
      &Annotations.create_for_group(group_hash, &1),
      "Error group not found.",
      params
    )
  end

  def group_index(conn, %{"group_hash" => group_hash}) do
    annotations = Annotations.list_for_group(group_hash)
    json(conn, %{annotations: Enum.map(annotations, &Annotations.format/1)})
  end

  defp do_create(conn, create_fn, not_found_msg, params) do
    attrs = %{
      "agent" => params["agent"],
      "action" => params["action"],
      "metadata" => params["metadata"]
    }

    case validate_required_fields(conn, attrs) do
      {:error, conn} ->
        conn

      :ok ->
        case create_fn.(attrs) do
          {:ok, annotation} ->
            conn |> put_status(201) |> json(Annotations.format(annotation))

          {:error, :not_found} ->
            CanaryWeb.Plugs.ProblemDetails.render_error(conn, 404, "not_found", not_found_msg)

          {:error, _changeset} ->
            CanaryWeb.Plugs.ProblemDetails.render_error(
              conn,
              422,
              "validation_error",
              "Invalid annotation."
            )
        end
    end
  end

  defp validate_required_fields(conn, attrs) do
    missing =
      ~w(agent action)
      |> Enum.filter(fn field -> is_nil(attrs[field]) or attrs[field] == "" end)

    if missing != [] do
      errors = Map.new(missing, fn field -> {field, ["is required"]} end)

      {:error,
       CanaryWeb.Plugs.ProblemDetails.render_error(
         conn,
         422,
         "validation_error",
         "Missing required fields.",
         %{errors: errors}
       )}
    else
      :ok
    end
  end
end
