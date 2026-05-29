//! Health-transition persistence.
//!
//! Target probes and monitor check-ins are different sources, but their
//! product contract is the same once state changes: update the health state,
//! append the deterministic timeline event, and correlate the health signal
//! into the incident graph in one SQLite transaction.

use canary_core::ids::{EventId, IncidentId};
use rusqlite::{OptionalExtension, params};
use serde_json::json;

use crate::{
    IncidentCorrelation, IncidentCorrelationEvent, Result, incidents::correlate_in_transaction,
};

/// Persisted event emitted by a health transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthTransitionCommit {
    /// Health transition event name.
    pub event: String,
    /// Service-event row id.
    pub id: String,
    /// Health event payload JSON.
    pub payload_json: String,
    /// Incident event emitted by correlation, when correlation changed state.
    pub incident_event: Option<IncidentCorrelationEvent>,
}

/// Health transition command for any health signal source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthTransition {
    /// HTTP target probe transition.
    Target(TargetHealthTransition),
    /// Non-HTTP monitor transition.
    Monitor(MonitorHealthTransition),
}

/// Target health transition command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetHealthTransition {
    /// Target id.
    pub target_id: String,
    /// Target display name.
    pub name: String,
    /// Service name resolved with Phoenix's target service fallback.
    pub service: String,
    /// Target URL.
    pub url: String,
    /// Previous target state.
    pub previous_state: String,
    /// New target state.
    pub state: String,
    /// Persisted consecutive failure counter.
    pub consecutive_failures: u32,
    /// Persisted consecutive success counter.
    pub consecutive_successes: u32,
    /// Whether this check was successful.
    pub check_succeeded: bool,
    /// Timestamp for state, event, and correlation writes.
    pub now: String,
    /// Service-event id for the health transition.
    pub event_id: EventId,
    /// Incident id to use if the transition opens an incident.
    pub incident_id: IncidentId,
    /// Service-event id to use if incident correlation emits an event.
    pub incident_event_id: EventId,
}

/// Monitor health transition command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorHealthTransition {
    /// Monitor id.
    pub monitor_id: String,
    /// Monitor display name.
    pub name: String,
    /// Service name resolved with Phoenix's monitor service fallback.
    pub service: String,
    /// Monitor mode, `schedule` or `ttl`.
    pub mode: String,
    /// Expected interval in milliseconds.
    pub expected_every_ms: i64,
    /// Grace period in milliseconds.
    pub grace_ms: i64,
    /// Previous monitor state.
    pub previous_state: String,
    /// New monitor state.
    pub state: String,
    /// Last check-in timestamp after this transition.
    pub last_check_in_at: Option<String>,
    /// Last check-in status after this transition.
    pub last_check_in_status: Option<String>,
    /// Deadline after this transition.
    pub deadline_at: Option<String>,
    /// Timestamp for state, event, and correlation writes.
    pub now: String,
    /// Service-event id for the health transition.
    pub event_id: EventId,
    /// Incident id to use if the transition opens an incident.
    pub incident_id: IncidentId,
    /// Service-event id to use if incident correlation emits an event.
    pub incident_event_id: EventId,
}

pub(crate) fn commit(
    connection: &mut rusqlite::Connection,
    transition: HealthTransition,
) -> Result<HealthTransitionCommit> {
    match transition {
        HealthTransition::Target(transition) => commit_target_transition(connection, transition),
        HealthTransition::Monitor(transition) => commit_monitor_transition(connection, transition),
    }
}

fn commit_target_transition(
    connection: &mut rusqlite::Connection,
    transition: TargetHealthTransition,
) -> Result<HealthTransitionCommit> {
    let transaction = connection.transaction()?;
    let sequence = next_sequence(
        &transaction,
        "target_state",
        "target_id",
        &transition.target_id,
    )?;
    upsert_target_state(&transaction, &transition, sequence)?;
    let payload_json = target_payload(&transition, sequence).to_string();
    let event = event_name_for(&transition.state).to_owned();
    let summary = format!(
        "{}: {} {}",
        transition.service, transition.name, transition.state
    );
    insert_service_event(
        &transaction,
        ServiceEventInsert {
            event_id: &transition.event_id,
            service: &transition.service,
            event: &event,
            entity_type: "target",
            entity_ref: &transition.target_id,
            state: &transition.state,
            summary: &summary,
            payload_json: &payload_json,
            now: &transition.now,
        },
    )?;
    let incident_event = correlate_in_transaction(
        &transaction,
        IncidentCorrelation {
            signal_type: "health_transition".to_owned(),
            signal_ref: transition.target_id.clone(),
            service: transition.service.clone(),
            incident_id: transition.incident_id,
            event_id: transition.incident_event_id,
            now: transition.now.clone(),
        },
    )?;
    transaction.commit()?;
    Ok(HealthTransitionCommit {
        event,
        id: transition.event_id.as_str().to_owned(),
        payload_json,
        incident_event,
    })
}

fn commit_monitor_transition(
    connection: &mut rusqlite::Connection,
    transition: MonitorHealthTransition,
) -> Result<HealthTransitionCommit> {
    let transaction = connection.transaction()?;
    let sequence = next_sequence(
        &transaction,
        "monitor_state",
        "monitor_id",
        &transition.monitor_id,
    )?;
    upsert_monitor_state(&transaction, &transition, sequence)?;
    let payload_json = monitor_payload(&transition, sequence).to_string();
    let event = event_name_for(&transition.state).to_owned();
    let summary = format!(
        "{}: {} {}",
        transition.service, transition.name, transition.state
    );
    insert_service_event(
        &transaction,
        ServiceEventInsert {
            event_id: &transition.event_id,
            service: &transition.service,
            event: &event,
            entity_type: "monitor",
            entity_ref: &transition.monitor_id,
            state: &transition.state,
            summary: &summary,
            payload_json: &payload_json,
            now: &transition.now,
        },
    )?;
    let incident_event = correlate_in_transaction(
        &transaction,
        IncidentCorrelation {
            signal_type: "health_transition".to_owned(),
            signal_ref: transition.monitor_id.clone(),
            service: transition.service.clone(),
            incident_id: transition.incident_id,
            event_id: transition.incident_event_id,
            now: transition.now.clone(),
        },
    )?;
    transaction.commit()?;
    Ok(HealthTransitionCommit {
        event,
        id: transition.event_id.as_str().to_owned(),
        payload_json,
        incident_event,
    })
}

fn next_sequence(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    id_column: &str,
    id: &str,
) -> Result<i64> {
    let sql = format!("SELECT sequence FROM {table} WHERE {id_column} = ?1");
    let current = transaction
        .query_row(&sql, [id], |row| row.get::<_, i64>(0))
        .optional()?
        .unwrap_or(0);
    Ok(current + 1)
}

fn upsert_target_state(
    transaction: &rusqlite::Transaction<'_>,
    transition: &TargetHealthTransition,
    sequence: i64,
) -> Result<()> {
    let last_success_at = transition
        .check_succeeded
        .then_some(transition.now.as_str());
    let last_failure_at = (!transition.check_succeeded).then_some(transition.now.as_str());
    transaction.execute(
        "INSERT INTO target_state (
            target_id, state, consecutive_failures, consecutive_successes,
            last_checked_at, last_success_at, last_failure_at, last_transition_at, sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?5, ?8)
         ON CONFLICT(target_id) DO UPDATE SET
            state = excluded.state,
            consecutive_failures = excluded.consecutive_failures,
            consecutive_successes = excluded.consecutive_successes,
            last_checked_at = excluded.last_checked_at,
            last_success_at = COALESCE(excluded.last_success_at, target_state.last_success_at),
            last_failure_at = COALESCE(excluded.last_failure_at, target_state.last_failure_at),
            last_transition_at = excluded.last_transition_at,
            sequence = excluded.sequence",
        params![
            transition.target_id,
            transition.state,
            transition.consecutive_failures,
            transition.consecutive_successes,
            transition.now,
            last_success_at,
            last_failure_at,
            sequence,
        ],
    )?;
    Ok(())
}

fn upsert_monitor_state(
    transaction: &rusqlite::Transaction<'_>,
    transition: &MonitorHealthTransition,
    sequence: i64,
) -> Result<()> {
    let last_success_at =
        matches!(transition.state.as_str(), "up").then_some(transition.now.as_str());
    let last_failure_at =
        matches!(transition.state.as_str(), "degraded" | "down").then_some(transition.now.as_str());
    transaction.execute(
        "INSERT INTO monitor_state (
            monitor_id, state, last_check_in_status, last_check_in_at,
            last_success_at, last_failure_at, deadline_at, first_missed_at,
            last_transition_at, sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9)
         ON CONFLICT(monitor_id) DO UPDATE SET
            state = excluded.state,
            last_check_in_status = COALESCE(excluded.last_check_in_status, monitor_state.last_check_in_status),
            last_check_in_at = COALESCE(excluded.last_check_in_at, monitor_state.last_check_in_at),
            last_success_at = COALESCE(excluded.last_success_at, monitor_state.last_success_at),
            last_failure_at = COALESCE(excluded.last_failure_at, monitor_state.last_failure_at),
            deadline_at = COALESCE(excluded.deadline_at, monitor_state.deadline_at),
            first_missed_at = CASE WHEN excluded.state = 'up' THEN NULL ELSE COALESCE(monitor_state.first_missed_at, ?8) END,
            last_transition_at = excluded.last_transition_at,
            sequence = excluded.sequence",
        params![
            transition.monitor_id,
            transition.state,
            transition.last_check_in_status,
            transition.last_check_in_at,
            last_success_at,
            last_failure_at,
            transition.deadline_at,
            transition.now,
            sequence,
        ],
    )?;
    Ok(())
}

struct ServiceEventInsert<'a> {
    event_id: &'a EventId,
    service: &'a str,
    event: &'a str,
    entity_type: &'a str,
    entity_ref: &'a str,
    state: &'a str,
    summary: &'a str,
    payload_json: &'a str,
    now: &'a str,
}

fn insert_service_event(
    transaction: &rusqlite::Transaction<'_>,
    event: ServiceEventInsert<'_>,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO service_events (
            id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            event.event_id.as_str(),
            event.service,
            event.event,
            event.entity_type,
            event.entity_ref,
            health_severity(event.state),
            event.summary,
            event.payload_json,
            event.now,
        ],
    )?;
    Ok(())
}

fn target_payload(transition: &TargetHealthTransition, sequence: i64) -> serde_json::Value {
    json!({
        "event": event_name_for(&transition.state),
        "target": {
            "name": transition.name,
            "service": transition.service,
            "url": transition.url,
        },
        "state": transition.state,
        "previous_state": transition.previous_state,
        "consecutive_failures": transition.consecutive_failures,
        "last_success_at": if transition.check_succeeded { Some(transition.now.as_str()) } else { None },
        "sequence": sequence,
        "timestamp": transition.now,
    })
}

fn monitor_payload(transition: &MonitorHealthTransition, sequence: i64) -> serde_json::Value {
    json!({
        "event": event_name_for(&transition.state),
        "monitor": {
            "name": transition.name,
            "service": transition.service,
            "mode": transition.mode,
            "expected_every_ms": transition.expected_every_ms,
            "grace_ms": transition.grace_ms,
        },
        "state": transition.state,
        "previous_state": transition.previous_state,
        "last_check_in_at": transition.last_check_in_at,
        "last_check_in_status": transition.last_check_in_status,
        "deadline_at": transition.deadline_at,
        "sequence": sequence,
        "timestamp": transition.now,
    })
}

fn event_name_for(state: &str) -> &'static str {
    match state {
        "up" => "health_check.recovered",
        "degraded" => "health_check.degraded",
        "down" => "health_check.down",
        _ => "health_check.updated",
    }
}

fn health_severity(state: &str) -> &'static str {
    match state {
        "down" => "error",
        "degraded" => "warning",
        _ => "info",
    }
}
