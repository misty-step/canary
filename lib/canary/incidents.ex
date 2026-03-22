defmodule Canary.Incidents do
  @moduledoc """
  Deterministic incident correlation across health transitions and error groups.
  """

  import Ecto.Query

  alias Canary.{ID, Repo}
  alias Canary.Schemas.{ErrorGroup, Incident, IncidentSignal, TargetState}

  @active_window_seconds 300
  @open_state "investigating"
  @resolved_state "resolved"

  @type signal_type :: :health_transition | :error_group

  @spec correlate(signal_type(), String.t(), String.t()) ::
          {:ok, Incident.t() | nil} | {:error, term()}
  def correlate(signal_type, signal_ref, service)
      when signal_type in [:health_transition, :error_group] and is_binary(signal_ref) and
             is_binary(service) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    case with_transaction(fn -> correlate_tx(signal_type, signal_ref, service, now) end) do
      {:ok, incident} -> {:ok, incident}
      other -> other
    end
  end

  defp correlate_tx(signal_type, signal_ref, service, now) do
    signal_kind = to_string(signal_type)
    signal_active? = signal_active?(signal_kind, signal_ref, now)
    incident = open_incident(service)

    cond do
      incident == nil and not signal_active? ->
        nil

      incident == nil ->
        create_incident(service, signal_kind, signal_ref, now)

      true ->
        update_incident(incident, signal_kind, signal_ref, signal_active?, now)
    end
  end

  defp create_incident(service, signal_type, signal_ref, now) do
    incident_id = ID.incident_id()

    incident =
      %Incident{id: incident_id}
      |> Incident.changeset(%{
        service: service,
        state: @open_state,
        severity: "medium",
        title: title_for(service),
        opened_at: now
      })
      |> Repo.insert!()

    %IncidentSignal{}
    |> IncidentSignal.changeset(%{
      incident_id: incident.id,
      signal_type: signal_type,
      signal_ref: signal_ref,
      attached_at: now
    })
    |> Repo.insert!()

    incident = refresh_incident!(incident.id)
    enqueue_webhook("incident.opened", incident, now)
    incident
  end

  defp update_incident(%Incident{} = incident, signal_type, signal_ref, signal_active?, now) do
    {signal_changed?, attached?} =
      sync_signal(incident.id, signal_type, signal_ref, signal_active?, now)

    normalized? = normalize_signals(incident.id, now)
    incident = refresh_incident!(incident.id)

    attrs = desired_incident_attrs(incident, now)
    incident_changed? = attrs != %{}

    incident =
      if incident_changed? do
        incident
        |> Incident.changeset(attrs)
        |> Repo.update!()
        |> Repo.preload(:signals)
      else
        incident
      end

    event =
      cond do
        incident.state == @resolved_state and
            (signal_changed? or normalized? or incident_changed?) ->
          "incident.resolved"

        attached? or signal_changed? or normalized? or incident_changed? ->
          "incident.updated"

        true ->
          nil
      end

    if event do
      enqueue_webhook(event, incident, now)
    end

    incident
  end

  defp desired_incident_attrs(%Incident{} = incident, now) do
    active_signals = Enum.reject(incident.signals, &resolved?/1)
    severity = desired_severity(active_signals, now)

    base =
      if active_signals == [] do
        %{state: @resolved_state, severity: severity, resolved_at: now}
      else
        %{state: @open_state, severity: severity, resolved_at: nil}
      end

    Enum.reduce(base, %{}, fn {field, value}, acc ->
      if Map.get(incident, field) == value do
        acc
      else
        Map.put(acc, field, value)
      end
    end)
  end

  defp desired_severity(active_signals, now) do
    recent_count =
      Enum.count(active_signals, fn signal ->
        within_active_window?(signal.attached_at, now)
      end)

    if recent_count >= 3, do: "high", else: "medium"
  end

  defp sync_signal(incident_id, signal_type, signal_ref, signal_active?, now) do
    signal =
      Repo.one(
        from(s in IncidentSignal,
          where:
            s.incident_id == ^incident_id and s.signal_type == ^signal_type and
              s.signal_ref == ^signal_ref
        )
      )

    cond do
      signal == nil and signal_active? ->
        %IncidentSignal{}
        |> IncidentSignal.changeset(%{
          incident_id: incident_id,
          signal_type: signal_type,
          signal_ref: signal_ref,
          attached_at: now
        })
        |> Repo.insert!()

        {true, true}

      signal == nil ->
        {false, false}

      signal_active? and not is_nil(signal.resolved_at) ->
        signal
        |> IncidentSignal.changeset(%{attached_at: now, resolved_at: nil})
        |> Repo.update!()

        {true, false}

      not signal_active? and is_nil(signal.resolved_at) ->
        signal
        |> IncidentSignal.changeset(%{resolved_at: now})
        |> Repo.update!()

        {true, false}

      true ->
        {false, false}
    end
  end

  defp normalize_signals(incident_id, now) do
    incident_id
    |> incident_signals()
    |> Enum.reduce(false, fn signal, changed? ->
      signal_active? = signal_active?(signal.signal_type, signal.signal_ref, now)

      cond do
        signal_active? and not is_nil(signal.resolved_at) ->
          signal
          |> IncidentSignal.changeset(%{resolved_at: nil, attached_at: now})
          |> Repo.update!()

          true

        not signal_active? and is_nil(signal.resolved_at) ->
          signal
          |> IncidentSignal.changeset(%{resolved_at: now})
          |> Repo.update!()

          true

        true ->
          changed?
      end
    end)
  end

  defp incident_signals(incident_id) do
    Repo.all(
      from(s in IncidentSignal,
        where: s.incident_id == ^incident_id,
        order_by: [asc: s.attached_at, asc: s.id]
      )
    )
  end

  defp open_incident(service) do
    Repo.one(
      from(i in Incident,
        where: i.service == ^service and i.state != ^@resolved_state,
        order_by: [desc: i.opened_at],
        limit: 1,
        preload: [signals: ^from(s in IncidentSignal, order_by: [asc: s.attached_at, asc: s.id])]
      )
    )
  end

  defp refresh_incident!(incident_id) do
    Repo.one!(
      from(i in Incident,
        where: i.id == ^incident_id,
        preload: [signals: ^from(s in IncidentSignal, order_by: [asc: s.attached_at, asc: s.id])]
      )
    )
  end

  defp signal_active?("health_transition", target_id, _now) do
    case Repo.get(TargetState, target_id) do
      %TargetState{state: "up"} -> false
      %TargetState{} -> true
      nil -> false
    end
  end

  defp signal_active?("error_group", group_hash, now) do
    case Repo.get(ErrorGroup, group_hash) do
      %ErrorGroup{status: "active", last_seen_at: last_seen_at} ->
        within_active_window?(last_seen_at, now)

      %ErrorGroup{} ->
        false

      nil ->
        false
    end
  end

  defp within_active_window?(timestamp, now) do
    with {:ok, timestamp_dt, _} <- DateTime.from_iso8601(timestamp),
         {:ok, now_dt, _} <- DateTime.from_iso8601(now) do
      DateTime.diff(now_dt, timestamp_dt, :second) <= @active_window_seconds
    else
      _ -> false
    end
  end

  defp resolved?(%IncidentSignal{resolved_at: nil}), do: false
  defp resolved?(%IncidentSignal{}), do: true

  defp title_for(service), do: "#{service} incident"

  defp enqueue_webhook(event, incident, now) do
    Canary.Workers.WebhookDelivery.enqueue_for_event(event, %{
      "event" => event,
      "incident" => incident_payload(incident),
      "timestamp" => now
    })
  end

  defp incident_payload(incident) do
    %{
      "id" => incident.id,
      "service" => incident.service,
      "state" => incident.state,
      "severity" => incident.severity,
      "title" => incident.title,
      "opened_at" => incident.opened_at,
      "resolved_at" => incident.resolved_at,
      "signals" =>
        Enum.map(incident.signals, fn signal ->
          %{
            "signal_type" => signal.signal_type,
            "signal_ref" => signal.signal_ref,
            "attached_at" => signal.attached_at,
            "resolved_at" => signal.resolved_at
          }
        end)
    }
  end

  defp with_transaction(fun) do
    if Repo.in_transaction?() do
      {:ok, fun.()}
    else
      Repo.transaction(fun)
    end
  end
end
