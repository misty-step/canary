defmodule Canary.Fixtures do
  @moduledoc "Shared test data builders."

  def clean_status_tables do
    Canary.Repo.delete_all(Canary.Schemas.Annotation)
    Canary.Repo.delete_all(Canary.Schemas.ServiceEvent)
    Canary.Repo.delete_all(Canary.Schemas.IncidentSignal)
    Canary.Repo.delete_all(Canary.Schemas.Incident)
    Canary.Repo.delete_all(Canary.Schemas.MonitorCheckIn)
    Canary.Repo.delete_all(Canary.Schemas.MonitorState)
    Canary.Repo.delete_all(Canary.Schemas.Monitor)
    Canary.Repo.delete_all(Canary.Schemas.TargetState)
    Canary.Repo.delete_all(Canary.Schemas.TargetCheck)
    Canary.Repo.delete_all(Canary.Schemas.Target)
    Canary.Repo.delete_all(Canary.Schemas.Error)
    Canary.Repo.delete_all(Canary.Schemas.ErrorGroup)
  end

  def create_target_with_state(name, state) do
    id = "TGT-#{name}"
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    Canary.Repo.insert!(%Canary.Schemas.Target{
      id: id,
      name: name,
      service: name,
      url: "https://#{name}.example.com/healthz",
      created_at: now
    })

    Canary.Repo.insert!(%Canary.Schemas.TargetState{
      target_id: id,
      state: state,
      consecutive_failures: if(state == "up", do: 0, else: 3),
      last_checked_at: now,
      last_success_at: if(state == "up", do: now, else: nil)
    })
  end

  def create_monitor_with_state(name, state, opts \\ []) do
    id = "MON-#{name}"
    now = Keyword.get(opts, :at, DateTime.utc_now()) |> DateTime.to_iso8601()
    expected_every_ms = Keyword.get(opts, :expected_every_ms, 60_000)
    mode = Keyword.get(opts, :mode, "ttl")

    Canary.Repo.insert!(%Canary.Schemas.Monitor{
      id: id,
      name: name,
      service: Keyword.get(opts, :service, name),
      mode: mode,
      expected_every_ms: expected_every_ms,
      grace_ms: Keyword.get(opts, :grace_ms, 0),
      created_at: now
    })

    Canary.Repo.insert!(%Canary.Schemas.MonitorState{
      monitor_id: id,
      state: state,
      last_check_in_status: if(state == "down", do: "error", else: "alive"),
      last_check_in_at: now,
      last_success_at: if(state == "down", do: nil, else: now),
      last_failure_at: if(state == "down", do: now, else: nil),
      deadline_at:
        DateTime.utc_now()
        |> DateTime.add(expected_every_ms, :millisecond)
        |> DateTime.to_iso8601(),
      last_transition_at: now,
      sequence: if(state == "unknown", do: 0, else: 1)
    })
  end

  def create_error_group(service, error_class, count, opts \\ [])
      when count > 0 do
    last_seen_at = Keyword.get(opts, :last_seen_at, DateTime.utc_now() |> DateTime.to_iso8601())
    id = "ERR-#{:crypto.strong_rand_bytes(8) |> Base.url_encode64(padding: false)}"

    group_hash =
      :crypto.hash(:sha256, "#{service}:#{error_class}") |> Base.encode16(case: :lower)

    Canary.Repo.insert!(%Canary.Schemas.ErrorGroup{
      group_hash: group_hash,
      service: service,
      error_class: error_class,
      severity: "error",
      status: "active",
      first_seen_at: last_seen_at,
      last_seen_at: last_seen_at,
      total_count: count,
      last_error_id: id
    })
  end

  def create_incident(service, opts \\ []) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()
    id = Canary.ID.incident_id()

    Canary.Repo.insert!(%Canary.Schemas.Incident{
      id: id,
      service: service,
      state: Keyword.get(opts, :state, "investigating"),
      severity: Keyword.get(opts, :severity, "medium"),
      opened_at: Keyword.get(opts, :opened_at, now)
    })
  end

  def create_annotation(target_type, target_id, attrs)
      when target_type in [:incident, :group, :target, :monitor] do
    id = Canary.ID.annotation_id()
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    {subject_type, legacy} =
      case target_type do
        :incident -> {"incident", %{incident_id: target_id}}
        :group -> {"error_group", %{group_hash: target_id}}
        :target -> {"target", %{}}
        :monitor -> {"monitor", %{}}
      end

    base = %{
      subject_type: subject_type,
      subject_id: target_id,
      agent: attrs[:agent] || "test-agent",
      action: attrs[:action] || "acknowledged",
      metadata: if(attrs[:metadata], do: Jason.encode!(attrs[:metadata])),
      created_at: now
    }

    %Canary.Schemas.Annotation{id: id}
    |> Canary.Schemas.Annotation.changeset(Map.merge(base, legacy))
    |> Canary.Repo.insert!()
  end
end
