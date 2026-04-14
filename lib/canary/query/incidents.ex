defmodule Canary.Query.Incidents do
  @moduledoc false

  alias Canary.Schemas.{ErrorGroup, Incident, IncidentSignal, TargetState}

  import Ecto.Query

  @incident_active_window_seconds 300

  @doc "Returns active incidents, optionally filtered by annotation."
  @spec active_incidents(keyword()) :: [map()]
  def active_incidents(opts \\ []) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    from(i in Incident,
      where: i.state != "resolved",
      order_by: [desc: i.opened_at],
      preload: [signals: ^from(s in IncidentSignal, order_by: [asc: s.attached_at, asc: s.id])]
    )
    |> Canary.Repos.read_repo().all()
    |> Enum.map(&active_incident_view(&1, now))
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
end
