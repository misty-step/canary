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
    case Annotations.list_for_incident(incident_id) do
      {:ok, annotations} ->
        json(conn, %{annotations: Enum.map(annotations, &Annotations.format/1)})

      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(conn, 404, "not_found", "Incident not found.")
    end
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
    case Annotations.list_for_group(group_hash) do
      {:ok, annotations} ->
        json(conn, %{annotations: Enum.map(annotations, &Annotations.format/1)})

      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "Error group not found."
        )
    end
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
