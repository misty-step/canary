defmodule CanaryWeb.AnnotationController do
  use CanaryWeb, :controller

  alias Canary.Annotations

  def create(conn, %{"incident_id" => incident_id} = params) do
    legacy_create(conn, &Annotations.create_for_incident(incident_id, &1), params, "Incident not found.")
  end

  def index(conn, %{"incident_id" => incident_id}) do
    render_list(conn, Annotations.list_for_incident(incident_id), "Incident not found.")
  end

  def group_create(conn, %{"group_hash" => group_hash} = params) do
    legacy_create(
      conn,
      &Annotations.create_for_group(group_hash, &1),
      params,
      "Error group not found."
    )
  end

  def group_index(conn, %{"group_hash" => group_hash}) do
    render_list(conn, Annotations.list_for_group(group_hash), "Error group not found.")
  end

  def unified_create(conn, params) do
    with :ok <- require_subject(params),
         :ok <- require_agent_action(params) do
      case Annotations.create(params) do
        {:ok, annotation} ->
          conn |> put_status(201) |> json(Annotations.format(annotation))

        {:error, :not_found} ->
          problem(conn, 404, "not_found", "Subject not found.")

        {:error, :invalid_subject_type} ->
          problem(conn, 422, "validation_error", "Unknown subject_type.", %{
            errors: %{subject_type: ["must be one of incident, error_group, target, monitor"]}
          })

        {:error, _} ->
          problem(conn, 422, "validation_error", "Invalid annotation.")
      end
    else
      {:error, {status, code, detail, extra}} ->
        problem(conn, status, code, detail, extra)
    end
  end

  def unified_index(conn, params) do
    with :ok <- require_subject(params) do
      case Annotations.list(params["subject_type"], params["subject_id"]) do
        {:ok, annotations} ->
          json(conn, %{annotations: Enum.map(annotations, &Annotations.format/1)})

        {:error, :not_found} ->
          problem(conn, 404, "not_found", "Subject not found.")

        {:error, :invalid_subject_type} ->
          problem(conn, 422, "validation_error", "Unknown subject_type.", %{
            errors: %{subject_type: ["must be one of incident, error_group, target, monitor"]}
          })
      end
    else
      {:error, {status, code, detail, extra}} ->
        problem(conn, status, code, detail, extra)
    end
  end

  defp legacy_create(conn, create_fn, params, not_found_msg) do
    with :ok <- require_agent_action(params) do
      case create_fn.(%{
             "agent" => params["agent"],
             "action" => params["action"],
             "metadata" => params["metadata"]
           }) do
        {:ok, annotation} ->
          conn |> put_status(201) |> json(Annotations.format(annotation))

        {:error, :not_found} ->
          problem(conn, 404, "not_found", not_found_msg)

        {:error, _} ->
          problem(conn, 422, "validation_error", "Invalid annotation.")
      end
    else
      {:error, {status, code, detail, extra}} ->
        problem(conn, status, code, detail, extra)
    end
  end

  defp render_list(conn, {:ok, annotations}, _not_found_msg) do
    json(conn, %{annotations: Enum.map(annotations, &Annotations.format/1)})
  end

  defp render_list(conn, {:error, :not_found}, not_found_msg) do
    problem(conn, 404, "not_found", not_found_msg)
  end

  defp require_subject(params) do
    type = params["subject_type"]
    id = params["subject_id"]

    cond do
      is_nil(type) or type == "" ->
        {:error,
         {422, "validation_error", "Missing subject.",
          %{errors: %{subject_type: ["is required"]}}}}

      is_nil(id) or id == "" ->
        {:error,
         {422, "validation_error", "Missing subject.",
          %{errors: %{subject_id: ["is required"]}}}}

      true ->
        :ok
    end
  end

  defp require_agent_action(params) do
    missing =
      ~w(agent action)
      |> Enum.filter(fn f -> is_nil(params[f]) or params[f] == "" end)

    case missing do
      [] ->
        :ok

      fields ->
        errors = Map.new(fields, fn f -> {f, ["is required"]} end)
        {:error, {422, "validation_error", "Missing required fields.", %{errors: errors}}}
    end
  end

  defp problem(conn, status, code, detail, extra \\ %{}) do
    CanaryWeb.Plugs.ProblemDetails.render_error(conn, status, code, detail, extra)
  end
end
