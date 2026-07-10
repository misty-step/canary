//! Fixture-backed parser tests for the Canary CLI.

use canary_cli::{
    dogfood_strict_failure_count, summarize_doctor, summarize_dogfood, summarize_error_detail,
    summarize_incident_detail, summarize_incidents, summarize_monitors, summarize_query,
    summarize_report, summarize_targets, summarize_timeline, summarize_webhook_delivery,
    tool_manifest,
};
use serde_json::{Value, json};

fn fixture(body: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(body)
}

#[test]
fn parses_report_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let value = fixture(include_str!("fixtures/report.json"))?;
    let lines = summarize_report(&value);
    assert!(lines.iter().any(|line| line == "targets: 1"));
    assert!(lines.iter().any(|line| line == "error_groups: 1"));
    // The SLI block is surfaced (no longer dropped), with the prior-window trajectory.
    assert!(lines.iter().any(|line| line
        == "service chrondle: slo=standard availability(targets)=0.950 availability(monitors)=n/a errors=7"));
    assert!(lines.iter().any(|line| line
        == "service chrondle trajectory: status=ok availability_delta(targets)=-0.040 availability_delta(monitors)=n/a errors_delta=+3"));
    Ok(())
}

#[test]
fn parses_query_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let value = fixture(include_str!("fixtures/query.json"))?;
    let lines = summarize_query(&value);
    assert!(lines.iter().any(|line| line == "service: chrondle"));
    assert!(lines.iter().any(|line| line == "total_errors: 7"));
    Ok(())
}

#[test]
fn parses_incidents_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let value = fixture(include_str!("fixtures/incidents.json"))?;
    let lines = summarize_incidents(&value);
    assert!(lines.iter().any(|line| line == "incidents: 1"));
    Ok(())
}

#[test]
fn summarizes_incident_detail_for_agent_loop() {
    let value = json!({
        "summary": "incident INC-loop: api incident",
        "action_brief": {
            "current_claim": {
                "id": "CLM-current",
                "owner": "codex",
                "state": "claimed"
            }
        },
        "incident": {
            "id": "INC-loop",
            "service": "api",
            "state": "investigating"
        },
        "signals": [{}],
        "annotations": [{}],
        "claims": [{
            "id": "CLM-latest",
            "owner": "codex",
            "state": "released"
        }],
        "recent_timeline_events": [{}, {}]
    });

    let lines = summarize_incident_detail(&value);
    assert!(lines.iter().any(|line| line == "incident_id: INC-loop"));
    assert!(lines.iter().any(|line| line == "annotations: 1"));
    assert!(lines.iter().any(|line| line == "timeline_events: 2"));
    assert!(
        lines
            .iter()
            .any(|line| line == "current_claim: CLM-current:codex:claimed")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "latest_claim: CLM-latest:codex:released")
    );
}

#[test]
fn parses_timeline_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let value = fixture(include_str!("fixtures/timeline.json"))?;
    let lines = summarize_timeline(&value);
    assert!(lines.iter().any(|line| line == "events: 2"));
    Ok(())
}

#[test]
fn summarizes_error_detail_with_stack_trace() {
    let value = json!({
        "summary": "error ERR-loop: api NullPointerException",
        "id": "ERR-loop",
        "service": "api",
        "error_class": "NullPointerException",
        "message": "boom",
        "message_template": null,
        "stack_trace": "at fn()\nat main()",
        "context": null,
        "severity": "error",
        "environment": "production",
        "group_hash": "GRP-abc",
        "created_at": "2026-06-14T02:07:53Z",
        "group": null,
        "incident_ids": ["INC-loop"]
    });

    let lines = summarize_error_detail(&value);
    assert!(lines.iter().any(|line| line == "id: ERR-loop"));
    assert!(lines.iter().any(|line| line == "stack_trace: present"));
    assert!(lines.iter().any(|line| line == "incident_ids: 1"));
}

#[test]
fn summarizes_webhook_delivery() {
    let value = json!({
        "delivery_id": "WHK-delivery-1",
        "webhook_id": "WHK-sub-1",
        "tenant_id": "TENANT-bootstrap",
        "project_id": "PROJECT-bootstrap",
        "service": "api",
        "event": "incident.opened",
        "status": "delivered",
        "attempt_count": 2,
        "reason": null,
        "first_attempt_at": "2026-06-14T02:07:00Z",
        "last_attempt_at": "2026-06-14T02:07:05Z",
        "delivered_at": "2026-06-14T02:07:05Z",
        "discarded_at": null,
        "completed_at": "2026-06-14T02:07:05Z",
        "created_at": "2026-06-14T02:06:55Z",
        "updated_at": "2026-06-14T02:07:05Z"
    });

    let lines = summarize_webhook_delivery(&value);
    assert!(
        lines
            .iter()
            .any(|line| line == "delivery_id: WHK-delivery-1")
    );
    assert!(lines.iter().any(|line| line == "status: delivered"));
    assert!(lines.iter().any(|line| line == "attempt_count: 2"));
}

#[test]
fn parses_targets_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let value = fixture(include_str!("fixtures/targets.json"))?;
    let lines = summarize_targets(&value);
    assert!(lines.iter().any(|line| line == "targets: 1"));
    Ok(())
}

#[test]
fn parses_monitors_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let value = fixture(include_str!("fixtures/monitors.json"))?;
    let lines = summarize_monitors(&value);
    assert!(lines.iter().any(|line| line == "monitors: 1"));
    Ok(())
}

#[test]
fn parses_dogfood_inventory_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let value = fixture(include_str!("fixtures/dogfood_inventory.json"))?;
    let lines = summarize_dogfood(&value);
    assert!(lines.iter().any(|line| line == "covered: 2"));
    assert!(lines.iter().any(|line| line == "strict_failures: 1"));
    assert_eq!(dogfood_strict_failure_count(&value), 1);
    Ok(())
}

#[test]
fn doctor_summary_includes_watchman_and_self_errors() {
    let value = json!({
        "endpoint": "https://canary.example",
        "key": "CANARY_API_KEY: redacted",
        "key_scope": "admin",
        "reachability": {
            "healthz": {"ok": true, "response": {"status": "ok"}},
            "readyz": {"ok": true, "response": {
                "status": "ready",
                "checks": {
                    "database": "ok",
                    "supervisor": "ok",
                    "workers": [
                        {"name": "webhook_delivery", "state": "started", "health": "ok", "last_success_at": "2026-06-14T02:07:53Z", "last_success_age_ms": 250, "failure_count": 0, "consecutive_failures": 0, "last_error_class": null, "due_count": 0, "in_flight_count": 0, "oldest_due_age_ms": null, "backoff_or_circuit_open": false},
                        {"name": "target_probe", "state": "started", "health": "ok", "last_success_at": "2026-06-14T02:07:55Z", "last_success_age_ms": 100, "failure_count": 0, "consecutive_failures": 0, "last_error_class": null, "due_count": 1, "in_flight_count": 0, "oldest_due_age_ms": 0, "backoff_or_circuit_open": false},
                        {"name": "monitor_overdue", "state": "started", "health": "ok", "last_success_at": "2026-06-14T02:07:55Z", "last_success_age_ms": 100, "failure_count": 0, "consecutive_failures": 0, "last_error_class": null, "due_count": 0, "in_flight_count": 0, "oldest_due_age_ms": null, "backoff_or_circuit_open": false},
                        {"name": "retention_prune", "state": "started", "health": "ok", "last_success_at": "2026-06-14T02:07:23Z", "last_success_age_ms": 32100, "failure_count": 0, "consecutive_failures": 0, "last_error_class": null, "due_count": 1, "in_flight_count": 0, "oldest_due_age_ms": null, "backoff_or_circuit_open": false},
                        {"name": "tls_scan", "state": "started", "health": "ok", "last_success_at": "2026-06-14T02:07:23Z", "last_success_age_ms": 32100, "failure_count": 0, "consecutive_failures": 0, "last_error_class": null, "due_count": 2, "in_flight_count": 0, "oldest_due_age_ms": null, "backoff_or_circuit_open": false}
                    ]
                }
            }}
        },
        "summary": {"ok": true, "summary": ["summary: Canary healthy"]},
        "services": {"ok": true, "summary": ["summary: all surfaces healthy"]},
        "witness": {
            "status": "observed",
            "monitor": "canary-watchman",
            "state": "up",
            "last_check_in_status": "alive",
            "last_check_in_at": "2026-06-12T00:00:00Z"
        },
        "canary_errors": {"ok": true, "summary": ["summary: 0 errors in canary in the last 1h."]},
        "incidents": {"ok": true, "summary": ["summary: 0 open incidents"]},
        "dr": {
            "status": {"ok": true, "stdout": "/data/canary.db: ok"},
            "restore_receipt": {"ok": true, "path": "docs/architecture/rust-cutover-evidence-2026-06-06.md"}
        },
        "dogfood": {"ok": true, "summary": ["covered: 4"]},
        "worker_readiness": {
            "available": true,
            "status": "ready",
            "workers": [
                {"name": "webhook_delivery", "state": "started", "health": "ok", "failure_count": 0},
                {"name": "target_probe", "state": "started", "health": "ok", "failure_count": 0},
                {"name": "monitor_overdue", "state": "started", "health": "ok", "failure_count": 0},
                {"name": "retention_prune", "state": "started", "health": "ok", "failure_count": 0},
                {"name": "tls_scan", "state": "started", "health": "ok", "failure_count": 0}
            ]
        },
        "alert_plane": {
            "available": true,
            "status": "healthy",
            "worker_count": 5,
            "impaired_workers": 0,
            "workers": [],
            "reasons": []
        }
    });

    let lines = summarize_doctor(&value);
    assert!(
        lines.iter().any(|line| line
            == "witness: canary-watchman up last_check_in=alive at 2026-06-12T00:00:00Z")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "canary_errors: summary: 0 errors in canary in the last 1h.")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "worker_readiness: ready 5 workers, 0 failing")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "alert_plane: healthy 5 workers")
    );
    assert!(
        lines.iter().any(|line| line
            == "dr: litestream ok, restore_receipt=docs/architecture/rust-cutover-evidence-2026-06-06.md")
    );
}

#[test]
fn doctor_summary_flags_missing_restore_receipt_and_worker_health_schema() {
    let value = json!({
        "endpoint": "https://canary.example",
        "key": "CANARY_API_KEY: redacted",
        "key_scope": "admin",
        "reachability": {
            "healthz": {"ok": true, "response": {"status": "ok"}},
            "readyz": {"ok": true, "response": {"status": "ready"}}
        },
        "summary": {"ok": true, "summary": ["summary: Canary healthy"]},
        "services": {"ok": true, "summary": ["summary: all surfaces healthy"]},
        "witness": {"status": "missing", "monitor": "canary-watchman"},
        "canary_errors": {"ok": true, "summary": ["summary: 0 errors in canary in the last 1h."]},
        "incidents": {"ok": true, "summary": ["summary: 0 open incidents"]},
        "dr": {
            "status": {"ok": true, "stdout": "/data/canary.db: ok"},
            "restore_receipt": {
                "ok": false,
                "path": "docs/backup-restore-dr.md",
                "reason": "no architecture DR receipt found"
            }
        },
        "dogfood": {"ok": true, "summary": ["covered: 4"]},
        "worker_readiness": {
            "available": true,
            "status": "ready",
            "worker_count": 1,
            "failing_workers": 1,
            "schema_missing_health_fields": 1,
            "workers": [
                {"name": "webhook_delivery", "state": "started", "failure_count": 0}
            ]
        },
        "alert_plane": {
            "available": true,
            "status": "impaired",
            "worker_count": 1,
            "impaired_workers": 1,
            "workers": [
                {"name": "webhook_delivery", "state": "started", "health": "unknown"}
            ],
            "reasons": ["webhook_delivery unknown"]
        }
    });

    let lines = summarize_doctor(&value);
    assert!(lines.iter().any(|line| line
        == "dr: litestream ok, restore_receipt_missing: no architecture DR receipt found, fallback=docs/backup-restore-dr.md"));
    assert!(lines.iter().any(
        |line| line == "worker_readiness: ready 1 workers, 1 failing, 1 missing health fields"
    ));
    assert!(
        lines
            .iter()
            .any(|line| line == "alert_plane: impaired 1 worker: webhook_delivery unknown")
    );
}

#[test]
fn doctor_watchman_down_fixture_stays_actionable() -> Result<(), Box<dyn std::error::Error>> {
    let value = fixture(include_str!("fixtures/doctor_watchman_down.json"))?;
    let lines = summarize_doctor(&value);

    assert_eq!(value["verdict"]["overall"], "degraded");
    assert_eq!(value["verdict"]["witness_age_ms"], 720000);
    assert_eq!(
        value["verdict"]["open_canary_incident"]["id"],
        "INC-witness"
    );
    assert!(
        value["verdict"]["next_operator_action"]
            .as_str()
            .unwrap_or_default()
            .contains("gh workflow run")
    );
    assert!(lines.iter().any(|line| line
        == "verdict: degraded; next: Run `gh workflow run \"Canary Witness\" --ref master`; then inspect the latest witness receipt and rerun `bin/canary doctor --json`."));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("canary-watchman down"))
    );

    Ok(())
}

#[test]
fn doctor_summary_reports_worker_pressure_separately() {
    let value = json!({
        "endpoint": "https://canary.example",
        "key": "CANARY_API_KEY: redacted",
        "key_scope": "admin",
        "reachability": {
            "healthz": {"ok": true, "response": {"status": "ok"}},
            "readyz": {"ok": true, "response": {"status": "ready"}}
        },
        "summary": {"ok": true, "summary": ["summary: Canary healthy"]},
        "services": {"ok": true, "summary": ["summary: all surfaces healthy"]},
        "witness": {"status": "observed", "monitor": "canary-watchman", "state": "up", "last_check_in_status": "alive", "last_check_in_at": "2026-06-15T22:00:00Z"},
        "canary_errors": {"ok": true, "summary": ["summary: 0 errors in canary in the last 1h."]},
        "incidents": {"ok": true, "summary": ["summary: 0 open incidents"]},
        "dr": {
            "status": {"ok": true, "stdout": "/data/canary.db: ok"},
            "restore_receipt": {"ok": true, "path": "docs/architecture/restore-drill-evidence-2026-06-14.md"}
        },
        "dogfood": {"ok": true, "summary": ["covered: 4"]},
        "worker_readiness": {
            "available": true,
            "status": "ready",
            "worker_count": 2,
            "failing_workers": 0,
            "pressured_workers": 1,
            "schema_missing_health_fields": 0,
            "workers": [
                {"name": "webhook_delivery", "state": "started", "health": "pressured", "failure_count": 0},
                {"name": "target_probe", "state": "started", "health": "ok", "failure_count": 0}
            ]
        },
        "alert_plane": {
            "available": true,
            "status": "impaired",
            "worker_count": 2,
            "impaired_workers": 1,
            "workers": [
                {"name": "webhook_delivery", "state": "started", "health": "pressured"}
            ],
            "reasons": ["webhook_delivery pressured"]
        },
        "verdict": {
            "overall": "degraded",
            "blocking_signals": ["alert-plane impaired: webhook_delivery pressured"],
            "next_operator_action": "Inspect alert-plane worker pressure and drain the named backlog before rerunning `bin/canary doctor --json`."
        }
    });

    let lines = summarize_doctor(&value);
    assert!(
        lines
            .iter()
            .any(|line| line == "worker_readiness: ready 2 workers, 0 failing, 1 pressured")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "alert_plane: impaired 1 worker: webhook_delivery pressured")
    );
    assert!(lines.iter().any(|line| line
        == "verdict: degraded; next: Inspect alert-plane worker pressure and drain the named backlog before rerunning `bin/canary doctor --json`."));
}

#[test]
fn mcp_manifest_exposes_operator_drilldowns() {
    let names = tool_manifest()
        .iter()
        .map(|tool| tool.name)
        .collect::<std::collections::BTreeSet<_>>();

    for name in [
        "canary_services",
        "canary_incidents",
        "canary_incident_get",
        "canary_timeline",
        "canary_targets",
        "canary_monitors",
        "canary_dogfood_audit",
        "canary_dogfood_value",
        "canary_witness",
        "canary_dr_status",
        "canary_error_get",
        "canary_webhook_delivery_get",
    ] {
        assert!(names.contains(name), "missing {name}");
    }
}

#[test]
fn checked_in_mcp_manifest_matches_generated_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let checked_in: Value =
        serde_json::from_str(include_str!("../../../priv/mcp/canary-cli-tools.json"))?;
    let generated = json!({
        "schema_version": 1,
        "tools": tool_manifest()
    });

    assert_eq!(checked_in, generated);
    Ok(())
}
