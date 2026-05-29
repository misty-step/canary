alias Canary.Repo

alias Canary.Schemas.{
  Annotation,
  Error,
  ErrorGroup,
  Incident,
  IncidentSignal,
  Monitor,
  MonitorState,
  ServiceEvent,
  Target,
  TargetState
}

Logger.configure(level: :warning)

Application.ensure_all_started(:ecto_sql)
Application.ensure_all_started(:ecto_sqlite3)
{:ok, _repo} = Repo.start_link()
Ecto.Adapters.SQL.Sandbox.mode(Repo, :auto)

now = "2026-05-28T20:00:00Z"
older = "2026-05-28T19:59:00Z"
newer = "2026-05-28T20:01:00Z"

insert_with_pk! = fn schema, pk, attrs ->
  schema.__struct__(pk)
  |> schema.changeset(attrs)
  |> Repo.insert!()
end

insert_with_id! = fn schema, id, attrs -> insert_with_pk!.(schema, [id: id], attrs) end

insert_with_id!.(Error, "ERR-readmodel0001", %{
  service: "ramp-api",
  error_class: "RuntimeError",
  message: "agent handoff failed",
  message_template: "agent handoff failed",
  stack_trace: "lib/ramp.ex:42: Ramp.run/1",
  context: ~s({"tenant":"alpha","run_id":"run-123"}),
  severity: "error",
  environment: "production",
  group_hash: "grp-readmodel-runtime",
  fingerprint: ~s(["ramp","handoff"]),
  region: "iad",
  classification_category: "application",
  classification_persistence: "persistent",
  classification_component: "runtime",
  created_at: now
})

insert_with_pk!.(ErrorGroup, [group_hash: "grp-readmodel-runtime"], %{
  service: "ramp-api",
  error_class: "RuntimeError",
  message_template: "agent handoff failed",
  severity: "error",
  first_seen_at: older,
  last_seen_at: now,
  total_count: 3,
  last_error_id: "ERR-readmodel0001",
  status: "active"
})

insert_with_id!.(Target, "TGT-readmodel-api", %{
  name: "Ramp API",
  service: "ramp-api",
  url: "https://ramp.example.com/healthz",
  method: "GET",
  interval_ms: 60_000,
  timeout_ms: 10_000,
  expected_status: "200",
  degraded_after: 1,
  down_after: 3,
  up_after: 1,
  active: 1,
  created_at: older
})

%TargetState{target_id: "TGT-readmodel-api"}
|> TargetState.changeset(%{
  target_id: "TGT-readmodel-api",
  state: "down",
  consecutive_failures: 4,
  consecutive_successes: 0,
  last_checked_at: now,
  last_failure_at: now,
  last_transition_at: now,
  sequence: 7
})
|> Repo.insert!()

insert_with_id!.(Monitor, "MON-readmodel-cron", %{
  name: "Ramp nightly import",
  service: "ramp-api",
  mode: "ttl",
  expected_every_ms: 60_000,
  grace_ms: 5_000,
  created_at: older
})

%MonitorState{monitor_id: "MON-readmodel-cron"}
|> MonitorState.changeset(%{
  monitor_id: "MON-readmodel-cron",
  state: "degraded",
  last_check_in_status: "alive",
  last_check_in_at: older,
  last_success_at: older,
  deadline_at: now,
  first_missed_at: now,
  last_transition_at: now,
  sequence: 4
})
|> Repo.insert!()

insert_with_id!.(Incident, "INC-readmodel0001", %{
  service: "ramp-api",
  state: "investigating",
  severity: "medium",
  title: "ramp-api needs agent attention",
  opened_at: older
})

for {signal_type, signal_ref, attached_at} <- [
      {"error_group", "grp-readmodel-runtime", older},
      {"health_transition", "TGT-readmodel-api", now},
      {"health_transition", "MON-readmodel-cron", newer}
    ] do
  %IncidentSignal{}
  |> IncidentSignal.changeset(%{
    incident_id: "INC-readmodel0001",
    signal_type: signal_type,
    signal_ref: signal_ref,
    attached_at: attached_at
  })
  |> Repo.insert!()
end

for row <- [
      {"ANN-readmodel-incident",
       %{
         subject_type: "incident",
         subject_id: "INC-readmodel0001",
         incident_id: "INC-readmodel0001",
         agent: "bb-sprite",
         action: "acknowledged",
         metadata: ~s({"note":"owner paged"}),
         created_at: newer
       }},
      {"ANN-readmodel-group",
       %{
         subject_type: "error_group",
         subject_id: "grp-readmodel-runtime",
         group_hash: "grp-readmodel-runtime",
         agent: "triage-agent",
         action: "triaged",
         metadata: ~s({"pr":"https://example.com/pr/1"}),
         created_at: now
       }},
      {"ANN-readmodel-target",
       %{
         subject_type: "target",
         subject_id: "TGT-readmodel-api",
         agent: "ops-agent",
         action: "investigating",
         metadata: ~s({"runbook":"https://example.com/runbook"}),
         created_at: now
       }}
    ] do
  {id, attrs} = row
  insert_with_id!.(Annotation, id, attrs)
end

insert_with_id!.(ServiceEvent, "EVT-readmodel-incident", %{
  service: "ramp-api",
  event: "incident.opened",
  entity_type: "incident",
  entity_ref: "INC-readmodel0001",
  severity: "medium",
  summary: "ramp-api: incident opened",
  payload: ~s({"event":"incident.opened","incident":{"id":"INC-readmodel0001"}}),
  created_at: now
})
