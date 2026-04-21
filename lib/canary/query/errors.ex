defmodule Canary.Query.Errors do
  @moduledoc false

  alias Canary.Schemas.{Error, ErrorGroup}

  import Ecto.Query

  @max_groups 50

  @spec errors_by_service(String.t(), String.t(), keyword()) ::
          {:ok, map()} | {:error, :invalid_window}
  def errors_by_service(service, window, opts \\ []) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      cursor = Keyword.get(opts, :cursor)

      query =
        from(g in ErrorGroup,
          where: g.service == ^service and g.last_seen_at >= ^cutoff,
          order_by: [desc: g.total_count, asc: g.group_hash],
          limit: ^@max_groups
        )

      query = apply_cursor(query, cursor)
      query = maybe_filter_annotation(query, opts)
      groups = query |> select_group_with_classification() |> Canary.Repos.read_repo().all()

      total = Enum.reduce(groups, 0, &(&1.total_count + &2))

      summary =
        Canary.Summary.error_query(%{
          total: total,
          service: service,
          window: window,
          groups: groups
        })

      {:ok,
       %{
         summary: summary,
         service: service,
         window: window,
         total_errors: total,
         groups: Enum.map(groups, &format_group/1),
         cursor: paginate_cursor(groups)
       }}
    end
  end

  @spec errors_by_error_class(String.t(), String.t(), keyword()) ::
          {:ok, map()} | {:error, :invalid_window}
  def errors_by_error_class(error_class, window, opts \\ []) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      cursor = Keyword.get(opts, :cursor)

      query =
        from(g in ErrorGroup,
          where: g.error_class == ^error_class and g.last_seen_at >= ^cutoff,
          order_by: [desc: g.total_count, asc: g.group_hash],
          limit: ^@max_groups
        )

      query =
        case Keyword.get(opts, :service) do
          nil -> query
          svc -> from(g in query, where: g.service == ^svc)
        end

      query = apply_cursor(query, cursor)
      query = maybe_filter_annotation(query, opts)
      groups = query |> select_group_with_classification() |> Canary.Repos.read_repo().all()
      total = Enum.reduce(groups, 0, &(&1.total_count + &2))

      summary =
        Canary.Summary.error_class_query(%{
          total: total,
          error_class: error_class,
          window: window,
          groups: groups
        })

      {:ok,
       %{
         summary: summary,
         error_class: error_class,
         window: window,
         total_errors: total,
         groups: Enum.map(groups, &format_group/1),
         cursor: paginate_cursor(groups)
       }}
    end
  end

  @spec errors_by_class(String.t()) :: {:ok, map()} | {:error, :invalid_window}
  def errors_by_class(window) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      groups =
        from(g in ErrorGroup,
          where: g.last_seen_at >= ^cutoff,
          select: %{
            error_class: g.error_class,
            total_count: sum(g.total_count),
            service_count: count(fragment("DISTINCT ?", g.service))
          },
          group_by: g.error_class,
          order_by: [desc: sum(g.total_count)],
          limit: ^@max_groups
        )
        |> Canary.Repos.read_repo().all()

      {total, class_count} = aggregate_by_class(cutoff)
      truncated = class_count > length(groups)

      summary =
        Canary.Summary.error_class_aggregate(%{
          total: total,
          class_count: class_count,
          window: window,
          groups: groups,
          truncated: truncated
        })

      {:ok,
       %{
         summary: summary,
         window: window,
         total_errors: total,
         total_error_classes: class_count,
         truncated: truncated,
         groups: groups
       }}
    end
  end

  defp aggregate_by_class(cutoff) do
    result =
      from(g in ErrorGroup,
        where: g.last_seen_at >= ^cutoff,
        select: %{
          total: coalesce(sum(g.total_count), 0),
          class_count: count(g.error_class, :distinct)
        }
      )
      |> Canary.Repos.read_repo().one()

    case result do
      %{total: total, class_count: class_count} -> {total || 0, class_count || 0}
      _ -> {0, 0}
    end
  end

  @spec error_detail(String.t()) :: {:ok, map()} | {:error, :not_found}
  def error_detail(error_id) do
    case Canary.Repos.read_repo().get(Error, error_id) do
      nil -> {:error, :not_found}
      error -> {:ok, build_error_detail(error)}
    end
  end

  @spec error_groups(String.t(), keyword()) ::
          {:ok, [map()]} | {:error, :invalid_window}
  def error_groups(window, opts \\ []) do
    now = Keyword.get(opts, :at, DateTime.utc_now())

    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window, now) do
      groups =
        from(g in ErrorGroup,
          where: g.last_seen_at >= ^cutoff and g.status == "active",
          order_by: [desc: g.total_count, asc: g.service, asc: g.error_class],
          limit: ^@max_groups
        )
        |> select_group_with_classification()
        |> Canary.Repos.read_repo().all()
        |> Enum.map(&format_group/1)

      {:ok, groups}
    end
  end

  @spec error_summary(String.t(), keyword()) ::
          {:ok, [map()]} | {:error, :invalid_window}
  def error_summary(window, opts \\ []) do
    now = Keyword.get(opts, :at, DateTime.utc_now())

    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window, now) do
      summary =
        from(g in ErrorGroup,
          where: g.last_seen_at >= ^cutoff and g.status == "active",
          group_by: g.service,
          select: %{
            service: g.service,
            total_count: sum(g.total_count),
            unique_classes: count(g.group_hash)
          },
          order_by: [desc: sum(g.total_count)]
        )
        |> Canary.Repos.read_repo().all()

      {:ok, summary}
    end
  end

  defp build_error_detail(error) do
    group = Canary.Repos.read_repo().get(ErrorGroup, error.group_hash)

    summary =
      Canary.Summary.error_detail(%{
        error_class: error.error_class,
        service: error.service,
        count: (group && group.total_count) || 1,
        first_seen: (group && group.first_seen_at) || error.created_at,
        last_seen: (group && group.last_seen_at) || error.created_at
      })

    %{
      summary: summary,
      id: error.id,
      service: error.service,
      error_class: error.error_class,
      message: error.message,
      message_template: error.message_template,
      stack_trace: error.stack_trace,
      context: safe_decode_json(error.context),
      severity: error.severity,
      environment: error.environment,
      group_hash: error.group_hash,
      created_at: error.created_at,
      group: group_summary(group)
    }
  end

  defp group_summary(nil), do: nil

  defp group_summary(group) do
    %{
      total_count: group.total_count,
      first_seen_at: group.first_seen_at,
      last_seen_at: group.last_seen_at,
      status: group.status
    }
  end

  # --- Annotation filters ---

  defp maybe_filter_annotation(query, opts) do
    query =
      case Keyword.get(opts, :with_annotation) do
        nil ->
          query

        action ->
          from(g in query,
            where:
              fragment(
                "EXISTS (SELECT 1 FROM annotations WHERE group_hash = ? AND action = ?)",
                g.group_hash,
                ^action
              )
          )
      end

    case Keyword.get(opts, :without_annotation) do
      nil ->
        query

      action ->
        from(g in query,
          where:
            fragment(
              "NOT EXISTS (SELECT 1 FROM annotations WHERE group_hash = ? AND action = ?)",
              g.group_hash,
              ^action
            )
        )
    end
  end

  # --- Shared formatters ---

  defp format_group(g) do
    %{
      group_hash: g.group_hash,
      error_class: g.error_class,
      service: g.service,
      count: g.total_count,
      first_seen: g.first_seen_at,
      last_seen: g.last_seen_at,
      sample_message: g.message_template,
      severity: g.severity,
      status: g.status,
      classification: format_classification(g)
    }
  end

  defp paginate_cursor(groups) do
    if length(groups) == @max_groups do
      last = List.last(groups)

      %{total_count: last.total_count, group_hash: last.group_hash}
      |> Jason.encode!()
      |> Base.url_encode64(padding: false)
    end
  end

  defp select_group_with_classification(query) do
    from(g in query,
      left_join: e in Error,
      on: e.id == g.last_error_id,
      select: %{
        group_hash: g.group_hash,
        error_class: g.error_class,
        service: g.service,
        total_count: g.total_count,
        first_seen_at: g.first_seen_at,
        last_seen_at: g.last_seen_at,
        message_template: g.message_template,
        severity: g.severity,
        status: g.status,
        classification_category: e.classification_category,
        classification_persistence: e.classification_persistence,
        classification_component: e.classification_component
      }
    )
  end

  defp format_classification(group) do
    %{
      category: classification_value(group, :classification_category),
      persistence: classification_value(group, :classification_persistence),
      component: classification_value(group, :classification_component)
    }
  end

  defp classification_value(group, key) do
    case Map.get(group, key) do
      value when value in [nil, ""] -> "unknown"
      value -> value
    end
  end

  defp apply_cursor(query, nil), do: query

  defp apply_cursor(query, cursor) do
    case decode_cursor(cursor) do
      {:ok, %{group_hash: after_hash, total_count: after_count}} ->
        from(g in query,
          where:
            g.total_count < ^after_count or
              (g.total_count == ^after_count and g.group_hash > ^after_hash)
        )

      {:ok, after_hash} when is_binary(after_hash) ->
        from(g in query, where: g.group_hash > ^after_hash)

      _ ->
        query
    end
  end

  defp decode_cursor(cursor) do
    with {:ok, json} <- Base.url_decode64(cursor, padding: false),
         {:ok, decoded} <- Jason.decode(json),
         {:ok, structured_cursor} <- validate_cursor(decoded) do
      {:ok, structured_cursor}
    else
      _ -> Base.decode64(cursor)
    end
  end

  defp validate_cursor(%{"group_hash" => group_hash, "total_count" => total_count})
       when is_binary(group_hash) and is_integer(total_count) do
    {:ok, %{group_hash: group_hash, total_count: total_count}}
  end

  defp validate_cursor(_decoded), do: :error

  defp safe_decode_json(nil), do: nil

  defp safe_decode_json(json) when is_binary(json) do
    case Jason.decode(json) do
      {:ok, decoded} -> decoded
      _ -> json
    end
  end

  defp safe_decode_json(other), do: other
end
