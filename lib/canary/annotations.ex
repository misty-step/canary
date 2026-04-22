defmodule Canary.Annotations do
  @moduledoc """
  Context for annotation CRUD. Annotations attach opaque consumer-authored
  metadata (PR links, ack tokens, fix references) to any signal-bearing
  subject: `incident`, `error_group`, `target`, `monitor`.

  Canary owns the substrate; consumers own the content. No action enum,
  no payload schema, no interpretation.
  """

  import Ecto.Query

  alias Canary.{ID, Repo}
  alias Canary.Schemas.{Annotation, ErrorGroup, Incident, Monitor, Target}

  @type subject_type :: String.t()

  @spec create(map()) :: {:ok, Annotation.t()} | {:error, term()}
  def create(attrs) when is_map(attrs) do
    with {:ok, {subject_type, subject_id}} <- parse_subject(attrs),
         :ok <- subject_exists(subject_type, subject_id) do
      build_and_insert(subject_type, subject_id, attrs)
    end
  end

  @spec list(subject_type(), String.t(), keyword()) ::
          {:ok, [Annotation.t()]} | {:error, :not_found | :invalid_subject_type}
  def list(subject_type, subject_id, opts \\ []) do
    with :ok <- validate_subject_type(subject_type),
         :ok <- subject_exists(subject_type, subject_id) do
      order = Keyword.get(opts, :order, :asc)

      rows =
        from(a in Annotation,
          where: a.subject_type == ^subject_type and a.subject_id == ^subject_id,
          order_by: ^order_by(order)
        )
        |> Canary.Repos.read_repo().all()

      {:ok, rows}
    end
  end

  @spec create_for_incident(String.t(), map()) :: {:ok, Annotation.t()} | {:error, term()}
  def create_for_incident(incident_id, attrs) do
    create(Map.merge(attrs, %{"subject_type" => "incident", "subject_id" => incident_id}))
  end

  @spec list_for_incident(String.t()) :: {:ok, [Annotation.t()]} | {:error, :not_found}
  def list_for_incident(incident_id), do: list("incident", incident_id)

  @spec create_for_group(String.t(), map()) :: {:ok, Annotation.t()} | {:error, term()}
  def create_for_group(group_hash, attrs) do
    create(Map.merge(attrs, %{"subject_type" => "error_group", "subject_id" => group_hash}))
  end

  @spec list_for_group(String.t()) :: {:ok, [Annotation.t()]} | {:error, :not_found}
  def list_for_group(group_hash), do: list("error_group", group_hash)

  @spec count_by_subject([{subject_type(), String.t()}]) :: %{
          {subject_type(), String.t()} => non_neg_integer()
        }
  def count_by_subject([]), do: %{}

  def count_by_subject(keys) when is_list(keys) do
    types = keys |> Enum.map(&elem(&1, 0)) |> Enum.uniq()
    ids = keys |> Enum.map(&elem(&1, 1)) |> Enum.uniq()
    wanted = MapSet.new(keys)

    from(a in Annotation,
      where: a.subject_type in ^types and a.subject_id in ^ids,
      group_by: [a.subject_type, a.subject_id],
      select: {a.subject_type, a.subject_id, count(a.id)}
    )
    |> Canary.Repos.read_repo().all()
    |> Enum.reduce(%{}, fn {type, id, count}, acc ->
      key = {type, id}
      if MapSet.member?(wanted, key), do: Map.put(acc, key, count), else: acc
    end)
  end

  @spec format(Annotation.t()) :: map()
  def format(%Annotation{} = ann) do
    %{
      id: ann.id,
      subject_type: ann.subject_type,
      subject_id: ann.subject_id,
      incident_id: ann.incident_id,
      group_hash: ann.group_hash,
      agent: ann.agent,
      action: ann.action,
      metadata: decode_metadata(ann.metadata),
      created_at: ann.created_at
    }
  end

  defp parse_subject(%{"subject_type" => type, "subject_id" => id})
       when is_binary(type) and is_binary(id) and id != "" do
    if type in Annotation.subject_types() do
      {:ok, {type, id}}
    else
      {:error, :invalid_subject_type}
    end
  end

  defp parse_subject(_), do: {:error, :invalid_subject}

  defp validate_subject_type(type) do
    if type in Annotation.subject_types(), do: :ok, else: {:error, :invalid_subject_type}
  end

  defp subject_exists("incident", id), do: exists?(Incident, :id, id)
  defp subject_exists("error_group", id), do: exists?(ErrorGroup, :group_hash, id)
  defp subject_exists("target", id), do: exists?(Target, :id, id)
  defp subject_exists("monitor", id), do: exists?(Monitor, :id, id)

  defp exists?(schema, key, value) do
    exists =
      from(r in schema, where: field(r, ^key) == ^value, select: 1, limit: 1)
      |> Canary.Repos.read_repo().one()

    if exists, do: :ok, else: {:error, :not_found}
  end

  defp build_and_insert(subject_type, subject_id, attrs) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    metadata =
      case attrs["metadata"] do
        nil -> nil
        m when is_map(m) -> Jason.encode!(m)
        m when is_binary(m) -> m
        _ -> nil
      end

    legacy = legacy_keys(subject_type, subject_id)

    %Annotation{id: ID.annotation_id()}
    |> Annotation.changeset(
      Map.merge(legacy, %{
        subject_type: subject_type,
        subject_id: subject_id,
        agent: attrs["agent"],
        action: attrs["action"],
        metadata: metadata,
        created_at: now
      })
    )
    |> Repo.insert()
  end

  defp legacy_keys("incident", id), do: %{incident_id: id}
  defp legacy_keys("error_group", hash), do: %{group_hash: hash}
  defp legacy_keys(_, _), do: %{}

  defp order_by(:asc), do: [asc: :created_at, asc: :id]
  defp order_by(:desc), do: [desc: :created_at, desc: :id]

  defp decode_metadata(nil), do: nil

  defp decode_metadata(json) when is_binary(json) do
    case Jason.decode(json) do
      {:ok, decoded} -> decoded
      _ -> json
    end
  end
end
