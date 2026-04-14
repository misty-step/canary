defmodule Canary.Query.Incidents do
  @moduledoc false

  alias Canary.Schemas.{ErrorGroup, Incident, IncidentSignal, TargetState}

  import Ecto.Query

  @incident_active_window_seconds 300

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

  defp load_signal_lookups(_repo, []), do: %{error_groups: %{}, target_states: %{}}

  defp load_signal_lookups(repo, incidents) do
    %{
      error_groups: load_error_groups(repo, incidents),
      target_states: load_target_states(repo, incidents)
    }
  end

  defp load_error_groups(repo, incidents) do
    incidents
    |> signal_refs("error_group")
    |> fetch_lookup(repo, ErrorGroup, :group_hash)
  end

  defp load_target_states(repo, incidents) do
    incidents
    |> signal_refs("health_transition")
    |> fetch_lookup(repo, TargetState, :target_id)
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
         %{target_states: target_states}
       ) do
    case Map.get(target_states, ref) do
      %TargetState{state: "up"} -> false
      %TargetState{} -> true
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
end
