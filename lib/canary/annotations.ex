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

  @default_page_limit 50
  @max_page_limit 50

  @spec create(map()) :: {:ok, Annotation.t()} | {:error, term()}
  def create(attrs) when is_map(attrs) do
    with {:ok, {subject_type, subject_id}} <- parse_subject(attrs),
         :ok <- subject_exists(subject_type, subject_id),
         {:ok, annotation} <- build_and_insert(subject_type, subject_id, attrs) do
      enqueue_webhook(annotation)
      {:ok, annotation}
    end
  end

  @spec list(subject_type(), String.t(), keyword()) ::
          {:ok, [Annotation.t()]} | {:error, :not_found | :invalid_subject_type}
  def list(subject_type, subject_id, opts \\ []) do
    with :ok <- validate_subject_type(subject_type),
         :ok <- subject_exists(subject_type, subject_id) do
      order = Keyword.get(opts, :order, :desc)

      rows =
        from(a in Annotation,
          where: a.subject_type == ^subject_type and a.subject_id == ^subject_id,
          order_by: ^order_by(order)
        )
        |> Canary.Repos.read_repo().all()

      {:ok, rows}
    end
  end

  @spec list_page(subject_type(), String.t(), keyword()) ::
          {:ok,
           %{
             summary: String.t(),
             annotations: [Annotation.t()],
             cursor: String.t() | nil
           }}
          | {:error, :not_found | :invalid_subject_type | :invalid_limit | :invalid_cursor}
  def list_page(subject_type, subject_id, opts \\ []) do
    with :ok <- validate_subject_type(subject_type),
         :ok <- subject_exists(subject_type, subject_id),
         {:ok, limit} <- parse_limit(Keyword.get(opts, :limit)),
         {:ok, cursor} <- decode_cursor(Keyword.get(opts, :cursor)) do
      repo = Canary.Repos.read_repo()

      rows =
        from(a in Annotation,
          where: a.subject_type == ^subject_type and a.subject_id == ^subject_id,
          order_by: [desc: a.created_at, desc: a.id]
        )
        |> maybe_apply_cursor(cursor)
        |> limit(^(limit + 1))
        |> repo.all()

      {page, next_cursor} = paginate(rows, limit)
      total = count_on_subject(subject_type, subject_id)

      summary =
        Canary.Summary.annotations_page(%{
          subject_type: subject_type,
          subject_id: subject_id,
          total_count: total,
          latest: latest_for_summary(subject_type, subject_id, page, cursor)
        })

      {:ok, %{summary: summary, annotations: page, cursor: next_cursor}}
    end
  end

  @spec count_on_subject(subject_type(), String.t()) :: non_neg_integer()
  def count_on_subject(subject_type, subject_id) do
    from(a in Annotation,
      where: a.subject_type == ^subject_type and a.subject_id == ^subject_id,
      select: count(a.id)
    )
    |> Canary.Repos.read_repo().one()
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

  defp enqueue_webhook(%Annotation{} = annotation) do
    payload = %{
      "event" => "annotation.added",
      "annotation" => format(annotation),
      "timestamp" => annotation.created_at
    }

    Canary.Workers.WebhookDelivery.enqueue_for_event("annotation.added", payload)
  end

  defp order_by(:asc), do: [asc: :created_at, asc: :id]
  defp order_by(:desc), do: [desc: :created_at, desc: :id]

  defp parse_limit(nil), do: {:ok, @default_page_limit}

  defp parse_limit(limit) when is_integer(limit) and limit > 0 and limit <= @max_page_limit,
    do: {:ok, limit}

  defp parse_limit(limit) when is_binary(limit) do
    case Integer.parse(limit) do
      {value, ""} when value > 0 and value <= @max_page_limit -> {:ok, value}
      _ -> {:error, :invalid_limit}
    end
  end

  defp parse_limit(_), do: {:error, :invalid_limit}

  defp decode_cursor(nil), do: {:ok, nil}
  defp decode_cursor(""), do: {:ok, nil}

  defp decode_cursor(cursor) when is_binary(cursor) do
    with {:ok, decoded} <- Base.url_decode64(cursor, padding: false),
         {:ok, %{"created_at" => created_at, "id" => id}} <- Jason.decode(decoded),
         true <- is_binary(created_at) and is_binary(id) do
      {:ok, %{created_at: created_at, id: id}}
    else
      _ -> {:error, :invalid_cursor}
    end
  end

  defp decode_cursor(_), do: {:error, :invalid_cursor}

  defp maybe_apply_cursor(query, nil), do: query

  defp maybe_apply_cursor(query, %{created_at: created_at, id: id}) do
    from(a in query,
      where:
        a.created_at < ^created_at or
          (a.created_at == ^created_at and a.id < ^id)
    )
  end

  defp paginate(rows, limit) do
    {page, rest} = Enum.split(rows, limit)

    next_cursor =
      case {rest, List.last(page)} do
        {[], _} -> nil
        {_, nil} -> nil
        {_, last} -> encode_cursor(last)
      end

    {page, next_cursor}
  end

  defp encode_cursor(%Annotation{} = row) do
    %{created_at: row.created_at, id: row.id}
    |> Jason.encode!()
    |> Base.url_encode64(padding: false)
  end

  defp latest_for_summary(_subject_type, _subject_id, [first | _], nil) do
    %{agent: first.agent, created_at: first.created_at}
  end

  defp latest_for_summary(subject_type, subject_id, _page, _cursor) do
    case from(a in Annotation,
           where: a.subject_type == ^subject_type and a.subject_id == ^subject_id,
           order_by: [desc: a.created_at, desc: a.id],
           limit: 1
         )
         |> Canary.Repos.read_repo().one() do
      nil -> nil
      %Annotation{agent: agent, created_at: created_at} -> %{agent: agent, created_at: created_at}
    end
  end

  defp decode_metadata(nil), do: nil

  defp decode_metadata(json) when is_binary(json) do
    case Jason.decode(json) do
      {:ok, decoded} -> decoded
      _ -> json
    end
  end
end
