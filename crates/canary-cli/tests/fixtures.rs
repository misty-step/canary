//! Fixture-backed parser tests for the Canary CLI.

use canary_cli::{
    dogfood_strict_failure_count, summarize_doctor, summarize_dogfood, summarize_incidents,
    summarize_monitors, summarize_query, summarize_report, summarize_targets, summarize_timeline,
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
fn parses_timeline_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let value = fixture(include_str!("fixtures/timeline.json"))?;
    let lines = summarize_timeline(&value);
    assert!(lines.iter().any(|line| line == "events: 2"));
    Ok(())
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
            "readyz": {"ok": true, "response": {"status": "ready"}}
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
        "dogfood": {"ok": true, "summary": ["covered: 4"]},
        "worker_readiness": {"available": false}
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
}
