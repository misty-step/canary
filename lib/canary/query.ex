defmodule Canary.Query do
  @moduledoc """
  Query API logic. Reads from errors, targets, and incidents.
  All responses include deterministic summary strings.
  """

  alias Canary.Schemas.{
    Error,
    ErrorGroup,
    Incident,
    IncidentSignal,
    Target,
    TargetCheck,
    TargetState
  }

  import Ecto.Query

  @incident_active_window_seconds 300
  @max_groups 50
  def errors_by_service(service, window, cursor \\ nil) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      query =
        from(g in ErrorGroup,
          where: g.service == ^service and g.last_seen_at >= ^cutoff,
          order_by: [desc: g.total_count],
          limit: ^@max_groups
        )

      query = apply_cursor(query, cursor)
      groups = Canary.Repos.read_repo().all(query)

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

  def errors_by_error_class(error_class, window, opts \\ []) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      cursor = Keyword.get(opts, :cursor)

      query =
        from(g in ErrorGroup,
          where: g.error_class == ^error_class and g.last_seen_at >= ^cutoff,
          order_by: [desc: g.total_count],
          limit: ^@max_groups
        )

      query =
        case Keyword.get(opts, :service) do
          nil -> query
          svc -> from(g in query, where: g.service == ^svc)
        end

      query = apply_cursor(query, cursor)
      groups = Canary.Repos.read_repo().all(query)
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
          limit: 50
        )
        |> Canary.Repos.read_repo().all()

      {:ok, %{window: window, groups: groups}}
    end
  end

  def error_detail(error_id) do
    case Canary.Repos.read_repo().get(Error, error_id) do
      nil -> {:error, :not_found}
      error -> {:ok, build_error_detail(error)}
    end
  end

  def search(query, opts \\ []) do
    case Keyword.get(opts, :window) do
      nil ->
        Canary.Query.Search.search(query, opts)

      window ->
        with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
          Canary.Query.Search.search(query, Keyword.put(opts, :cutoff, cutoff))
        end
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

  @recent_checks_limit 5

  def health_targets do
    repo = Canary.Repos.read_repo()

    targets_with_state =
      from(t in Target,
        left_join: s in TargetState,
        on: t.id == s.target_id,
        order_by: t.name,
        select: {t, s}
      )
      |> repo.all()

    target_ids = Enum.map(targets_with_state, fn {t, _} -> t.id end)

    checks_by_target = fetch_recent_checks(repo, target_ids)

    Enum.map(targets_with_state, fn {target, state} ->
      recent = Map.get(checks_by_target, target.id, [])

      %{
        id: target.id,
        name: target.name,
        url: target.url,
        state: (state && state.state) || "unknown",
        consecutive_failures: (state && state.consecutive_failures) || 0,
        last_checked_at: state && state.last_checked_at,
        last_success_at: state && state.last_success_at,
        latency_ms: recent |> List.first() |> then(&(&1 && &1.latency_ms)),
        tls_expires_at: Enum.find_value(recent, & &1.tls_expires_at),
        recent_checks:
          Enum.map(recent, fn c ->
            %{
              checked_at: c.checked_at,
              result: c.result,
              status_code: c.status_code,
              latency_ms: c.latency_ms
            }
          end)
      }
    end)
  end

  def health_status do
    targets = health_targets()
    summary = Canary.Summary.health_status(%{targets: targets})
    %{summary: summary, targets: targets}
  end

  def error_groups(window) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      {:ok, error_groups_since(cutoff)}
    end
  end

  def error_summary(window) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      {:ok, error_summary_since(cutoff)}
    end
  end

  def recent_transitions(window) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      {:ok, recent_transitions_since(cutoff)}
    end
  end

  def report_slice(window) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      {:ok,
       %{
         error_groups: error_groups_since(cutoff),
         error_summary: error_summary_since(cutoff),
         incidents: active_incidents(),
         recent_transitions: recent_transitions_since(cutoff)
       }}
    end
  end

  # Batch-fetch top-N recent checks per target using ROW_NUMBER window function.
  # 1 query replaces N individual queries.
  defp fetch_recent_checks(_repo, []), do: %{}

  defp fetch_recent_checks(repo, target_ids) do
    ranked =
      from(c in TargetCheck,
        where: c.target_id in ^target_ids,
        select: %{
          target_id: c.target_id,
          checked_at: c.checked_at,
          result: c.result,
          status_code: c.status_code,
          latency_ms: c.latency_ms,
          tls_expires_at: c.tls_expires_at,
          rn: over(row_number(), :w)
        },
        windows: [w: [partition_by: c.target_id, order_by: [desc: c.checked_at]]]
      )

    from(r in subquery(ranked),
      where: r.rn <= ^@recent_checks_limit,
      order_by: [asc: r.target_id, asc: r.rn]
    )
    |> repo.all()
    |> Enum.group_by(& &1.target_id)
  end

  def target_checks(target_id, window) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      checks =
        from(c in TargetCheck,
          where: c.target_id == ^target_id and c.checked_at >= ^cutoff,
          order_by: [desc: c.checked_at],
          limit: 500
        )
        |> Canary.Repos.read_repo().all()

      {:ok, checks}
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
      status: g.status
    }
  end

  defp paginate_cursor(groups) do
    if length(groups) == @max_groups do
      groups |> List.last() |> Map.get(:group_hash) |> Base.encode64()
    end
  end

  # --- Helpers ---

  defp error_groups_since(cutoff) do
    from(g in ErrorGroup,
      where: g.last_seen_at >= ^cutoff and g.status == "active",
      order_by: [desc: g.total_count, asc: g.service, asc: g.error_class],
      limit: ^@max_groups
    )
    |> Canary.Repos.read_repo().all()
    |> Enum.map(&format_group/1)
  end

  defp error_summary_since(cutoff) do
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
  end

  defp recent_transitions_since(cutoff) do
    from(t in Target,
      join: s in TargetState,
      on: t.id == s.target_id,
      where: s.last_transition_at >= ^cutoff,
      order_by: [desc: s.last_transition_at, asc: t.name],
      select: %{
        target_id: t.id,
        target_name: t.name,
        state: s.state,
        transitioned_at: s.last_transition_at
      }
    )
    |> Canary.Repos.read_repo().all()
  end

  defp active_incidents do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    from(i in Incident,
      where: i.state != "resolved",
      order_by: [desc: i.opened_at],
      preload: [signals: ^from(s in IncidentSignal, order_by: [asc: s.attached_at, asc: s.id])]
    )
    |> Canary.Repos.read_repo().all()
    |> Enum.map(&active_incident_view(&1, now))
    |> Enum.reject(&is_nil/1)
  end

  defp active_incident_view(incident, now) do
    active_signals =
      Enum.filter(incident.signals, fn signal ->
        signal_active_for_report?(signal, now)
      end)

    case active_signals do
      [] ->
        nil

      signals ->
        format_incident(incident, signals, now)
    end
  end

  defp format_incident(incident, signals, now) do
    %{
      id: incident.id,
      service: incident.service,
      state: "investigating",
      severity: incident_severity(signals, now),
      title: incident.title,
      opened_at: incident.opened_at,
      resolved_at: incident.resolved_at,
      signal_count: length(signals),
      signals:
        Enum.map(signals, fn signal ->
          %{
            signal_type: signal.signal_type,
            signal_ref: signal.signal_ref,
            attached_at: signal.attached_at,
            resolved_at: signal.resolved_at
          }
        end)
    }
  end

  defp signal_active_for_report?(%IncidentSignal{resolved_at: resolved_at}, _now)
       when not is_nil(resolved_at),
       do: false

  defp signal_active_for_report?(
         %IncidentSignal{signal_type: "health_transition", signal_ref: ref},
         _now
       ) do
    case Canary.Repos.read_repo().get(TargetState, ref) do
      %TargetState{state: "up"} -> false
      %TargetState{} -> true
      nil -> false
    end
  end

  defp signal_active_for_report?(
         %IncidentSignal{signal_type: "error_group", signal_ref: ref},
         now
       ) do
    case Canary.Repos.read_repo().get(ErrorGroup, ref) do
      %ErrorGroup{status: "active", last_seen_at: last_seen_at} ->
        within_incident_window?(last_seen_at, now)

      %ErrorGroup{} ->
        false

      nil ->
        false
    end
  end

  defp signal_active_for_report?(%IncidentSignal{}, _now), do: false

  defp incident_severity(signals, now) do
    recent_count =
      Enum.count(signals, fn signal ->
        within_incident_window?(signal.attached_at, now)
      end)

    if recent_count >= 3, do: "high", else: "medium"
  end

  defp within_incident_window?(timestamp, now) do
    with {:ok, timestamp_dt, _} <- DateTime.from_iso8601(timestamp),
         {:ok, now_dt, _} <- DateTime.from_iso8601(now) do
      DateTime.diff(now_dt, timestamp_dt, :second) <= @incident_active_window_seconds
    else
      _ -> false
    end
  end

  defp apply_cursor(query, nil), do: query

  defp apply_cursor(query, cursor) do
    case Base.decode64(cursor) do
      {:ok, after_hash} ->
        from(g in query, where: g.group_hash > ^after_hash)

      _ ->
        query
    end
  end

  defp safe_decode_json(nil), do: nil

  defp safe_decode_json(json) when is_binary(json) do
    case Jason.decode(json) do
      {:ok, decoded} -> decoded
      _ -> json
    end
  end

  defp safe_decode_json(other), do: other
end
