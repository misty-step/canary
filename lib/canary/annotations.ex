defmodule Canary.Annotations do
  @moduledoc "Context for annotation CRUD on incidents and error groups."

  import Ecto.Query

  alias Canary.{ID, Repo}
  alias Canary.Schemas.{Annotation, ErrorGroup, Incident}

  @spec create_for_incident(String.t(), map()) :: {:ok, Annotation.t()} | {:error, term()}
  def create_for_incident(incident_id, attrs) do
    case Canary.Repos.read_repo().get(Incident, incident_id) do
      nil ->
        {:error, :not_found}

      _incident ->
        build_and_insert(%{incident_id: incident_id}, attrs)
    end
  end

  @spec list_for_incident(String.t()) :: [Annotation.t()]
  def list_for_incident(incident_id) do
    from(a in Annotation,
      where: a.incident_id == ^incident_id,
      order_by: [asc: a.created_at, asc: a.id]
    )
    |> Canary.Repos.read_repo().all()
  end

  @spec create_for_group(String.t(), map()) :: {:ok, Annotation.t()} | {:error, term()}
  def create_for_group(group_hash, attrs) do
    case Canary.Repos.read_repo().get(ErrorGroup, group_hash) do
      nil ->
        {:error, :not_found}

      _group ->
        build_and_insert(%{group_hash: group_hash}, attrs)
    end
  end

  @spec list_for_group(String.t()) :: [Annotation.t()]
  def list_for_group(group_hash) do
    from(a in Annotation,
      where: a.group_hash == ^group_hash,
      order_by: [asc: a.created_at, asc: a.id]
    )
    |> Canary.Repos.read_repo().all()
  end

  @spec format(Annotation.t()) :: map()
  def format(%Annotation{} = ann) do
    %{
      id: ann.id,
      incident_id: ann.incident_id,
      group_hash: ann.group_hash,
      agent: ann.agent,
      action: ann.action,
      metadata: decode_metadata(ann.metadata),
      created_at: ann.created_at
    }
  end

  defp decode_metadata(nil), do: nil

  defp decode_metadata(json) when is_binary(json) do
    case Jason.decode(json) do
      {:ok, decoded} -> decoded
      _ -> json
    end
  end

  defp build_and_insert(target, attrs) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    metadata =
      case attrs["metadata"] do
        nil -> nil
        m when is_map(m) -> Jason.encode!(m)
        m when is_binary(m) -> m
        _ -> nil
      end

    %Annotation{id: ID.annotation_id()}
    |> Annotation.changeset(
      Map.merge(target, %{
        agent: attrs["agent"],
        action: attrs["action"],
        metadata: metadata,
        created_at: now
      })
    )
    |> Repo.insert()
  end
end
