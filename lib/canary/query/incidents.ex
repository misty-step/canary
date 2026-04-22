defmodule Canary.Query.Incidents do
  @moduledoc false

  alias Canary.Schemas.{
    Annotation,
    ErrorGroup,
    Incident,
    IncidentSignal,
    Monitor,
    MonitorState,
    ServiceEvent,
    Target,
    TargetState
  }

  import Ecto.Query

  @incident_active_window_seconds 300
  @max_signals 25
  @max_annotations 20
  @max_timeline_events 5

  @doc "Returns active incidents, optionally filtered by annotation."
  @spec active_incidents(keyword()) :: [map()]
  def active_incidents(opts \\ []) do
    now = Keyword.get(opts, :at, DateTime.utc_now())
    repo = Canary.Repos.read_repo()

    incidents =
      from(i in Incident,
        where: i.state != "resolved",
        order_by: [desc: i.opened_at],
        preload: [signals: ^from(s in IncidentSignal, order_by: [asc: s.attached_at, asc: s.id])]
      )
      |> repo.all()

    signal_lookups = load_signal_lookups(repo, incidents)

    incidents
    |> Enum.map(&active_incident_view(&1, now, signal_lookups))
    |> Enum.reject(&is_nil/1)
    |> maybe_filter_incident_annotation(opts)
  end

  defp maybe_filter_incident_annotation(incidents, opts) do
    with_action = Keyword.get(opts, :with_annotation)
    without_action = Keyword.get(opts, :without_annotation)

    if is_nil(with_action) and is_nil(without_action) do
      incidents
    else
      ids = Enum.map(incidents, & &1.id)

      incidents
      |> then(fn incs ->
        if with_action do
          have = annotated_incident_ids(ids, with_action)
          Enum.filter(incs, &(&1.id in have))
        else
          incs
        end
      end)
      |> then(fn incs ->
        if without_action do
          have = annotated_incident_ids(ids, without_action)
          Enum.reject(incs, &(&1.id in have))
        else
          incs
        end
      end)
    end
  end

  defp annotated_incident_ids([], _action), do: []

  defp annotated_incident_ids(incident_ids, action) do
    from(a in Canary.Schemas.Annotation,
      where: a.incident_id in ^incident_ids and a.action == ^action,
      select: a.incident_id,
      distinct: true
    )
    |> Canary.Repos.read_repo().all()
  end

  defp active_incident_view(incident, now, signal_lookups) do
    active_signals =
      Enum.filter(incident.signals, fn signal ->
        signal_active_for_report?(signal, now, signal_lookups)
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

  defp load_signal_lookups(_repo, []), do: %{error_groups: %{}, health_states: %{}}

  defp load_signal_lookups(repo, incidents) do
    %{
      error_groups: load_error_groups(repo, incidents),
      health_states: load_health_states(repo, incidents)
    }
  end

  defp load_error_groups(repo, incidents) do
    incidents
    |> signal_refs("error_group")
    |> fetch_lookup(repo, ErrorGroup, :group_hash)
  end

  defp load_health_states(repo, incidents) do
    refs = signal_refs(incidents, "health_transition")

    refs
    |> fetch_lookup(repo, TargetState, :target_id)
    |> Map.merge(fetch_lookup(refs, repo, MonitorState, :monitor_id))
  end

  defp signal_refs(incidents, signal_type) do
    incidents
    |> Enum.flat_map(& &1.signals)
    |> Enum.filter(&(&1.signal_type == signal_type))
    |> Enum.map(& &1.signal_ref)
    |> Enum.uniq()
  end

  defp fetch_lookup([], _repo, _schema, _key), do: %{}

  defp fetch_lookup(refs, repo, schema, key) do
    from(record in schema,
      where: field(record, ^key) in ^refs
    )
    |> repo.all()
    |> Map.new(&{Map.fetch!(&1, key), &1})
  end

  defp signal_active_for_report?(
         %IncidentSignal{resolved_at: resolved_at},
         _now,
         _signal_lookups
       )
       when not is_nil(resolved_at),
       do: false

  defp signal_active_for_report?(
         %IncidentSignal{signal_type: "health_transition", signal_ref: ref},
         _now,
         %{health_states: health_states}
       ) do
    case Map.get(health_states, ref) do
      %{state: "up"} -> false
      %{} -> true
      nil -> false
    end
  end

  defp signal_active_for_report?(
         %IncidentSignal{signal_type: "error_group", signal_ref: ref},
         now,
         %{error_groups: error_groups}
       ) do
    case Map.get(error_groups, ref) do
      %ErrorGroup{status: "active", last_seen_at: last_seen_at} ->
        within_incident_window?(last_seen_at, now)

      %ErrorGroup{} ->
        false

      nil ->
        false
    end
  end

  defp signal_active_for_report?(%IncidentSignal{}, _now, _signal_lookups), do: false

  defp incident_severity(signals, now) do
    recent_count =
      Enum.count(signals, fn signal ->
        within_incident_window?(signal.attached_at, now)
      end)

    if recent_count >= 3, do: "high", else: "medium"
  end

  defp within_incident_window?(timestamp, %DateTime{} = now) do
    case DateTime.from_iso8601(timestamp) do
      {:ok, timestamp_dt, _} ->
        DateTime.diff(now, timestamp_dt, :second) <= @incident_active_window_seconds

      _ ->
        false
    end
  end

  @doc "Returns a single incident with bounded signals, annotations, and timeline context."
  @spec detail(String.t(), keyword()) :: {:ok, map()} | {:error, :not_found}
  def detail(incident_id, _opts \\ []) do
    repo = Canary.Repos.read_repo()

    case repo.get(Incident, incident_id) do
      nil ->
        {:error, :not_found}

      incident ->
        total_signal_count = count_signals(repo, incident_id)
        signals = fetch_top_signals(repo, incident_id, @max_signals)
        signals_truncated = total_signal_count > length(signals)

        signal_context = load_signal_context(repo, signals)
        annotation_counts = load_signal_annotation_counts(signals)
        context_with_counts = Map.put(signal_context, :annotation_counts, annotation_counts)
        formatted_signals = Enum.map(signals, &format_signal(&1, context_with_counts))

        {annotations, annotations_truncated} = load_annotations(repo, incident_id)
        recent_timeline_events = load_recent_timeline_events(repo, incident_id)

        incident_view = format_incident_for_detail(incident, total_signal_count)

        summary =
          Canary.Summary.incident_detail(%{
            incident: incident_view,
            signal_count: total_signal_count,
            annotation_count: length(annotations)
          })

        {:ok,
         %{
           summary: summary,
           incident: incident_view,
           signals: formatted_signals,
           signals_truncated: signals_truncated,
           annotations: annotations,
           annotations_truncated: annotations_truncated,
           recent_timeline_events: recent_timeline_events
         }}
    end
  end

  defp count_signals(repo, incident_id) do
    from(s in IncidentSignal,
      where: s.incident_id == ^incident_id,
      select: count(s.id)
    )
    |> repo.one()
  end

  defp fetch_top_signals(repo, incident_id, limit) do
    from(s in IncidentSignal,
      where: s.incident_id == ^incident_id,
      order_by: [desc: s.attached_at, desc: s.id],
      limit: ^limit
    )
    |> repo.all()
  end

  defp format_incident_for_detail(%Incident{} = incident, total_signal_count) do
    %{
      id: incident.id,
      service: incident.service,
      state: incident.state,
      severity: incident.severity,
      title: incident.title,
      opened_at: incident.opened_at,
      resolved_at: incident.resolved_at,
      signal_count: total_signal_count
    }
  end

  defp load_signal_annotation_counts([]), do: %{}

  defp load_signal_annotation_counts(signals) do
    signals
    |> Enum.map(&signal_subject_key/1)
    |> Enum.reject(&is_nil/1)
    |> Enum.uniq()
    |> Canary.Annotations.count_by_subject()
  end

  defp signal_subject_key(%IncidentSignal{signal_type: "error_group", signal_ref: ref})
       when is_binary(ref),
       do: {"error_group", ref}

  defp signal_subject_key(%IncidentSignal{signal_type: "health_transition", signal_ref: ref})
       when is_binary(ref) do
    cond do
      String.starts_with?(ref, "TGT-") -> {"target", ref}
      String.starts_with?(ref, "MON-") -> {"monitor", ref}
      true -> nil
    end
  end

  defp signal_subject_key(_), do: nil

  defp load_signal_context(_repo, []),
    do: %{error_groups: %{}, target_states: %{}, targets: %{}, monitor_states: %{}, monitors: %{}}

  defp load_signal_context(repo, signals) do
    error_refs = refs_by_type(signals, "error_group")
    health_refs = refs_by_type(signals, "health_transition")
    {target_refs, monitor_refs} = Enum.split_with(health_refs, &String.starts_with?(&1, "TGT-"))

    %{
      error_groups: lookup_by(repo, ErrorGroup, :group_hash, error_refs),
      target_states: lookup_by(repo, TargetState, :target_id, target_refs),
      targets: lookup_by(repo, Target, :id, target_refs),
      monitor_states: lookup_by(repo, MonitorState, :monitor_id, monitor_refs),
      monitors: lookup_by(repo, Monitor, :id, monitor_refs)
    }
  end

  defp refs_by_type(signals, type) do
    signals
    |> Enum.filter(&(&1.signal_type == type))
    |> Enum.map(& &1.signal_ref)
    |> Enum.uniq()
  end

  defp lookup_by(_repo, _schema, _key, []), do: %{}

  defp lookup_by(repo, schema, key, refs) do
    from(r in schema, where: field(r, ^key) in ^refs)
    |> repo.all()
    |> Map.new(&{Map.fetch!(&1, key), &1})
  end

  defp format_signal(
         %IncidentSignal{signal_type: "error_group"} = signal,
         %{error_groups: groups} = context
       ) do
    group = Map.get(groups, signal.signal_ref)

    base = %{
      type: "error_group",
      group_hash: signal.signal_ref,
      attached_at: signal.attached_at,
      resolved_at: signal.resolved_at,
      annotation_count: annotation_count(signal, context)
    }

    case group do
      %ErrorGroup{} = g ->
        Map.merge(base, %{
          summary: error_group_signal_summary(g, signal),
          error_class: g.error_class,
          total_count: g.total_count,
          first_seen_at: g.first_seen_at,
          last_seen_at: g.last_seen_at
        })

      _ ->
        Map.merge(base, %{
          summary: "Error group #{truncate_hash(signal.signal_ref)} (detail unavailable).",
          error_class: nil,
          total_count: nil,
          first_seen_at: nil,
          last_seen_at: nil
        })
    end
  end

  defp format_signal(%IncidentSignal{signal_type: "health_transition"} = signal, context) do
    ref = signal.signal_ref

    cond do
      String.starts_with?(ref, "TGT-") ->
        format_target_signal(signal, context)

      String.starts_with?(ref, "MON-") ->
        format_monitor_signal(signal, context)

      true ->
        %{
          type: "health_transition",
          summary: "Health transition on #{ref} (detail unavailable).",
          signal_ref: ref,
          attached_at: signal.attached_at,
          resolved_at: signal.resolved_at,
          annotation_count: 0
        }
    end
  end

  defp format_signal(%IncidentSignal{} = signal, context) do
    %{
      type: signal.signal_type,
      summary: "Signal of type #{signal.signal_type} on #{signal.signal_ref}.",
      signal_ref: signal.signal_ref,
      attached_at: signal.attached_at,
      resolved_at: signal.resolved_at,
      annotation_count: annotation_count(signal, context)
    }
  end

  defp annotation_count(signal, context) do
    counts = Map.get(context, :annotation_counts, %{})

    case signal_subject_key(signal) do
      nil -> 0
      key -> Map.get(counts, key, 0)
    end
  end

  defp format_target_signal(signal, %{target_states: states, targets: targets} = context) do
    state = Map.get(states, signal.signal_ref)
    target = Map.get(targets, signal.signal_ref)

    state_label = (state && state.state) || "unknown"
    consecutive_failures = (state && state.consecutive_failures) || 0
    name = (target && target.name) || signal.signal_ref

    summary_text =
      case signal.resolved_at do
        nil ->
          "Target #{name} is #{state_label} (#{consecutive_failures} consecutive #{pluralize(consecutive_failures, "failure", "failures")})."

        _ ->
          "Target #{name} recovered to #{state_label}."
      end

    %{
      type: "health_transition",
      summary: summary_text,
      target_id: signal.signal_ref,
      target_name: name,
      current_state: state_label,
      consecutive_failures: consecutive_failures,
      attached_at: signal.attached_at,
      resolved_at: signal.resolved_at,
      annotation_count: annotation_count(signal, context)
    }
  end

  defp format_monitor_signal(signal, %{monitor_states: states, monitors: monitors} = context) do
    state = Map.get(states, signal.signal_ref)
    monitor = Map.get(monitors, signal.signal_ref)

    state_label = (state && state.state) || "unknown"
    name = (monitor && monitor.name) || signal.signal_ref

    summary_text =
      case signal.resolved_at do
        nil -> "Monitor #{name} is #{state_label}."
        _ -> "Monitor #{name} recovered to #{state_label}."
      end

    %{
      type: "health_transition",
      summary: summary_text,
      monitor_id: signal.signal_ref,
      monitor_name: name,
      current_state: state_label,
      attached_at: signal.attached_at,
      resolved_at: signal.resolved_at,
      annotation_count: annotation_count(signal, context)
    }
  end

  defp error_group_signal_summary(%ErrorGroup{} = g, _signal) do
    "#{g.total_count} #{pluralize(g.total_count, "occurrence", "occurrences")} of #{g.error_class} (last seen #{g.last_seen_at})."
  end

  defp load_annotations(repo, incident_id) do
    rows =
      from(a in Annotation,
        where: a.incident_id == ^incident_id,
        order_by: [desc: a.created_at, desc: a.id],
        limit: ^(@max_annotations + 1)
      )
      |> repo.all()

    truncated = length(rows) > @max_annotations
    annotations = rows |> Enum.take(@max_annotations) |> Enum.map(&Canary.Annotations.format/1)
    {annotations, truncated}
  end

  defp load_recent_timeline_events(repo, incident_id) do
    from(e in ServiceEvent,
      where: e.entity_type == "incident" and e.entity_ref == ^incident_id,
      order_by: [desc: e.created_at, desc: e.id],
      limit: @max_timeline_events
    )
    |> repo.all()
    |> Enum.map(&format_timeline_event/1)
  end

  defp format_timeline_event(event) do
    %{
      id: event.id,
      event: event.event,
      severity: event.severity,
      summary: event.summary,
      created_at: event.created_at
    }
  end

  defp truncate_hash(hash) when is_binary(hash) and byte_size(hash) > 12 do
    String.slice(hash, 0, 12) <> "..."
  end

  defp truncate_hash(hash), do: hash

  defp pluralize(n, singular, plural), do: Canary.Summary.pluralize(n, singular, plural)
end
