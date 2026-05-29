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

/// Persisted outcome of one target probe observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetProbeCommitResult {
    /// Health transition emitted by this probe, when the probe changed state.
    pub transition: Option<HealthTransitionCommit>,
}

/// Persisted outcome of one monitor check-in observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorCheckInCommitResult {
    /// Health transition emitted by this check-in, when it changed state.
    pub transition: Option<HealthTransitionCommit>,
}

/// One observed HTTP target probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetProbeCommit {
    /// Target id.
    pub target_id: String,
    /// Persisted state after the probe.
    pub state: String,
    /// Persisted consecutive failure counter after the probe.
    pub consecutive_failures: u32,
    /// Persisted consecutive success counter after the probe.
    pub consecutive_successes: u32,
    /// Whether this check was successful.
    pub check_succeeded: bool,
    /// Concrete target probe row to persist.
    pub check: TargetCheckObservation,
    /// Timestamp for check and state writes.
    pub now: String,
    /// Transition metadata, present only when this probe changed state.
    pub transition: Option<TargetTransitionEvent>,
}

/// Target transition metadata emitted by a probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetTransitionEvent {
    /// Target display name.
    pub name: String,
    /// Service name resolved with Phoenix's target service fallback.
    pub service: String,
    /// Target URL.
    pub url: String,
    /// Previous target state.
    pub previous_state: String,
    /// Service-event id for the health transition.
    pub event_id: EventId,
    /// Incident id to use if the transition opens an incident.
    pub incident_id: IncidentId,
    /// Service-event id to use if incident correlation emits an event.
    pub incident_event_id: EventId,
}

/// Observed HTTP target probe persisted with a target transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCheckObservation {
    /// HTTP status code returned by the target, when a response was received.
    pub status_code: Option<i64>,
    /// Probe latency in milliseconds.
    pub latency_ms: Option<i64>,
    /// Phoenix-compatible probe result, for example `ok` or `error`.
    pub result: String,
    /// TLS certificate expiration timestamp, when known.
    pub tls_expires_at: Option<String>,
    /// Probe error detail, when the probe failed before a valid response.
    pub error_detail: Option<String>,
    /// Probe region.
    pub region: Option<String>,
}

/// One observed non-HTTP monitor check-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorCheckInCommit {
    /// Monitor id.
    pub monitor_id: String,
    /// Persisted state after the check-in.
    pub state: String,
    /// Last check-in timestamp after this check-in.
    pub last_check_in_at: Option<String>,
    /// Last check-in status after this check-in.
    pub last_check_in_status: Option<String>,
    /// Deadline after this check-in.
    pub deadline_at: Option<String>,
    /// Concrete monitor check-in row to persist.
    pub check_in: MonitorCheckInObservation,
    /// Timestamp for state writes.
    pub now: String,
    /// Transition metadata, present only when this check-in changed state.
    pub transition: Option<MonitorTransitionEvent>,
}

/// Monitor transition metadata emitted by a check-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorTransitionEvent {
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
    /// Service-event id for the health transition.
    pub event_id: EventId,
    /// Incident id to use if the transition opens an incident.
    pub incident_id: IncidentId,
    /// Service-event id to use if incident correlation emits an event.
    pub incident_event_id: EventId,
}

/// Observed non-HTTP monitor check-in persisted with a monitor transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorCheckInObservation {
    /// Check-in id.
    pub id: String,
    /// Caller supplied idempotency key or external check-in id.
    pub external_id: Option<String>,
    /// Phoenix-compatible check-in status.
    pub status: String,
    /// Observed timestamp.
    pub observed_at: String,
    /// TTL supplied by this check-in, when present.
    pub ttl_ms: Option<i64>,
    /// Human-readable check-in summary.
    pub summary: Option<String>,
    /// JSON context string.
    pub context: Option<String>,
}

pub(crate) fn commit_target_probe(
    connection: &mut rusqlite::Connection,
    probe: TargetProbeCommit,
) -> Result<TargetProbeCommitResult> {
    let transaction = connection.transaction()?;
    let sequence = match &probe.transition {
        Some(_) => next_sequence(&transaction, "target_state", "target_id", &probe.target_id)?,
        None => current_sequence(&transaction, "target_state", "target_id", &probe.target_id)?,
    };
    upsert_target_state_fields(
        &transaction,
        TargetStateWrite {
            target_id: &probe.target_id,
            state: &probe.state,
            consecutive_failures: probe.consecutive_failures,
            consecutive_successes: probe.consecutive_successes,
            check_succeeded: probe.check_succeeded,
            now: &probe.now,
            transitioned: probe.transition.is_some(),
        },
        sequence,
    )?;
    insert_target_check(&transaction, &probe.target_id, &probe.now, &probe.check)?;

    let transition = if let Some(event) = &probe.transition {
        Some(record_target_transition(
            &transaction,
            TargetTransitionRecord {
                target_id: &probe.target_id,
                name: &event.name,
                service: &event.service,
                url: &event.url,
                previous_state: &event.previous_state,
                state: &probe.state,
                consecutive_failures: probe.consecutive_failures,
                check_succeeded: probe.check_succeeded,
                now: &probe.now,
                event_id: &event.event_id,
                incident_id: event.incident_id.clone(),
                incident_event_id: event.incident_event_id.clone(),
                sequence,
            },
        )?)
    } else {
        None
    };

    transaction.commit()?;
    Ok(TargetProbeCommitResult { transition })
}

pub(crate) fn commit_monitor_check_in(
    connection: &mut rusqlite::Connection,
    check_in: MonitorCheckInCommit,
) -> Result<MonitorCheckInCommitResult> {
    let transaction = connection.transaction()?;
    let sequence = match &check_in.transition {
        Some(_) => next_sequence(
            &transaction,
            "monitor_state",
            "monitor_id",
            &check_in.monitor_id,
        )?,
        None => current_sequence(
            &transaction,
            "monitor_state",
            "monitor_id",
            &check_in.monitor_id,
        )?,
    };
    upsert_monitor_state_fields(
        &transaction,
        MonitorStateWrite {
            monitor_id: &check_in.monitor_id,
            state: &check_in.state,
            last_check_in_at: check_in.last_check_in_at.as_deref(),
            last_check_in_status: check_in.last_check_in_status.as_deref(),
            deadline_at: check_in.deadline_at.as_deref(),
            now: &check_in.now,
            transitioned: check_in.transition.is_some(),
        },
        sequence,
    )?;
    insert_monitor_check_in(&transaction, &check_in.monitor_id, &check_in.check_in)?;

    let transition = if let Some(event) = &check_in.transition {
        Some(record_monitor_transition(
            &transaction,
            MonitorTransitionRecord {
                monitor_id: &check_in.monitor_id,
                name: &event.name,
                service: &event.service,
                mode: &event.mode,
                expected_every_ms: event.expected_every_ms,
                grace_ms: event.grace_ms,
                previous_state: &event.previous_state,
                state: &check_in.state,
                last_check_in_at: check_in.last_check_in_at.as_deref(),
                last_check_in_status: check_in.last_check_in_status.as_deref(),
                deadline_at: check_in.deadline_at.as_deref(),
                now: &check_in.now,
                event_id: &event.event_id,
                incident_id: event.incident_id.clone(),
                incident_event_id: event.incident_event_id.clone(),
                sequence,
            },
        )?)
    } else {
        None
    };

    transaction.commit()?;
    Ok(MonitorCheckInCommitResult { transition })
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

fn current_sequence(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    id_column: &str,
    id: &str,
) -> Result<i64> {
    let sql = format!("SELECT sequence FROM {table} WHERE {id_column} = ?1");
    Ok(transaction
        .query_row(&sql, [id], |row| row.get::<_, i64>(0))
        .optional()?
        .unwrap_or(0))
}

struct TargetStateWrite<'a> {
    target_id: &'a str,
    state: &'a str,
    consecutive_failures: u32,
    consecutive_successes: u32,
    check_succeeded: bool,
    now: &'a str,
    transitioned: bool,
}

fn upsert_target_state_fields(
    transaction: &rusqlite::Transaction<'_>,
    write: TargetStateWrite<'_>,
    sequence: i64,
) -> Result<()> {
    let last_success_at = write.check_succeeded.then_some(write.now);
    let last_failure_at = (!write.check_succeeded).then_some(write.now);
    let last_transition_at = write.transitioned.then_some(write.now);
    transaction.execute(
        "INSERT INTO target_state (
            target_id, state, consecutive_failures, consecutive_successes,
            last_checked_at, last_success_at, last_failure_at, last_transition_at, sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(target_id) DO UPDATE SET
            state = excluded.state,
            consecutive_failures = excluded.consecutive_failures,
            consecutive_successes = excluded.consecutive_successes,
            last_checked_at = excluded.last_checked_at,
            last_success_at = COALESCE(excluded.last_success_at, target_state.last_success_at),
            last_failure_at = COALESCE(excluded.last_failure_at, target_state.last_failure_at),
            last_transition_at = COALESCE(excluded.last_transition_at, target_state.last_transition_at),
            sequence = excluded.sequence",
        params![
            write.target_id,
            write.state,
            write.consecutive_failures,
            write.consecutive_successes,
            write.now,
            last_success_at,
            last_failure_at,
            last_transition_at,
            sequence,
        ],
    )?;
    Ok(())
}

struct MonitorStateWrite<'a> {
    monitor_id: &'a str,
    state: &'a str,
    last_check_in_at: Option<&'a str>,
    last_check_in_status: Option<&'a str>,
    deadline_at: Option<&'a str>,
    now: &'a str,
    transitioned: bool,
}

fn upsert_monitor_state_fields(
    transaction: &rusqlite::Transaction<'_>,
    write: MonitorStateWrite<'_>,
    sequence: i64,
) -> Result<()> {
    let last_success_at =
        matches!(write.last_check_in_status, Some("alive" | "ok")).then_some(write.now);
    let last_failure_at = matches!(write.last_check_in_status, Some("error")).then_some(write.now);
    let last_transition_at = write.transitioned.then_some(write.now);
    let first_missed_at = (write.transitioned && write.state != "up").then_some(write.now);
    transaction.execute(
        "INSERT INTO monitor_state (
            monitor_id, state, last_check_in_status, last_check_in_at,
            last_success_at, last_failure_at, deadline_at, first_missed_at,
            last_transition_at, sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(monitor_id) DO UPDATE SET
            state = excluded.state,
            last_check_in_status = COALESCE(excluded.last_check_in_status, monitor_state.last_check_in_status),
            last_check_in_at = COALESCE(excluded.last_check_in_at, monitor_state.last_check_in_at),
            last_success_at = COALESCE(excluded.last_success_at, monitor_state.last_success_at),
            last_failure_at = COALESCE(excluded.last_failure_at, monitor_state.last_failure_at),
            deadline_at = COALESCE(excluded.deadline_at, monitor_state.deadline_at),
            first_missed_at = CASE WHEN excluded.state = 'up' THEN NULL ELSE COALESCE(monitor_state.first_missed_at, excluded.first_missed_at) END,
            last_transition_at = COALESCE(excluded.last_transition_at, monitor_state.last_transition_at),
            sequence = excluded.sequence",
        params![
            write.monitor_id,
            write.state,
            write.last_check_in_status,
            write.last_check_in_at,
            last_success_at,
            last_failure_at,
            write.deadline_at,
            first_missed_at,
            last_transition_at,
            sequence,
        ],
    )?;
    Ok(())
}

struct TargetTransitionRecord<'a> {
    target_id: &'a str,
    name: &'a str,
    service: &'a str,
    url: &'a str,
    previous_state: &'a str,
    state: &'a str,
    consecutive_failures: u32,
    check_succeeded: bool,
    now: &'a str,
    event_id: &'a EventId,
    incident_id: IncidentId,
    incident_event_id: EventId,
    sequence: i64,
}

fn record_target_transition(
    transaction: &rusqlite::Transaction<'_>,
    transition: TargetTransitionRecord<'_>,
) -> Result<HealthTransitionCommit> {
    let payload_json = target_payload(&transition).to_string();
    let event = event_name_for(transition.state).to_owned();
    let summary = format!(
        "{}: {} {}",
        transition.service, transition.name, transition.state
    );
    insert_service_event(
        transaction,
        ServiceEventInsert {
            event_id: transition.event_id,
            service: transition.service,
            event: &event,
            entity_type: "target",
            entity_ref: transition.target_id,
            state: transition.state,
            summary: &summary,
            payload_json: &payload_json,
            now: transition.now,
        },
    )?;
    let incident_event = correlate_in_transaction(
        transaction,
        IncidentCorrelation {
            signal_type: "health_transition".to_owned(),
            signal_ref: transition.target_id.to_owned(),
            service: transition.service.to_owned(),
            incident_id: transition.incident_id,
            event_id: transition.incident_event_id,
            now: transition.now.to_owned(),
        },
    )?;
    Ok(HealthTransitionCommit {
        event,
        id: transition.event_id.as_str().to_owned(),
        payload_json,
        incident_event,
    })
}

struct MonitorTransitionRecord<'a> {
    monitor_id: &'a str,
    name: &'a str,
    service: &'a str,
    mode: &'a str,
    expected_every_ms: i64,
    grace_ms: i64,
    previous_state: &'a str,
    state: &'a str,
    last_check_in_at: Option<&'a str>,
    last_check_in_status: Option<&'a str>,
    deadline_at: Option<&'a str>,
    now: &'a str,
    event_id: &'a EventId,
    incident_id: IncidentId,
    incident_event_id: EventId,
    sequence: i64,
}

fn record_monitor_transition(
    transaction: &rusqlite::Transaction<'_>,
    transition: MonitorTransitionRecord<'_>,
) -> Result<HealthTransitionCommit> {
    let payload_json = monitor_payload(&transition).to_string();
    let event = event_name_for(transition.state).to_owned();
    let summary = format!(
        "{}: {} {}",
        transition.service, transition.name, transition.state
    );
    insert_service_event(
        transaction,
        ServiceEventInsert {
            event_id: transition.event_id,
            service: transition.service,
            event: &event,
            entity_type: "monitor",
            entity_ref: transition.monitor_id,
            state: transition.state,
            summary: &summary,
            payload_json: &payload_json,
            now: transition.now,
        },
    )?;
    let incident_event = correlate_in_transaction(
        transaction,
        IncidentCorrelation {
            signal_type: "health_transition".to_owned(),
            signal_ref: transition.monitor_id.to_owned(),
            service: transition.service.to_owned(),
            incident_id: transition.incident_id,
            event_id: transition.incident_event_id,
            now: transition.now.to_owned(),
        },
    )?;
    Ok(HealthTransitionCommit {
        event,
        id: transition.event_id.as_str().to_owned(),
        payload_json,
        incident_event,
    })
}

fn insert_target_check(
    transaction: &rusqlite::Transaction<'_>,
    target_id: &str,
    checked_at: &str,
    check: &TargetCheckObservation,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO target_checks (
            target_id, checked_at, status_code, latency_ms, result,
            tls_expires_at, error_detail, region
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            target_id,
            checked_at,
            check.status_code,
            check.latency_ms,
            check.result,
            check.tls_expires_at,
            check.error_detail,
            check.region,
        ],
    )?;
    Ok(())
}

fn insert_monitor_check_in(
    transaction: &rusqlite::Transaction<'_>,
    monitor_id: &str,
    check_in: &MonitorCheckInObservation,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO monitor_check_ins (
            id, monitor_id, external_id, status, observed_at, ttl_ms, summary, context
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            check_in.id,
            monitor_id,
            check_in.external_id,
            check_in.status,
            check_in.observed_at,
            check_in.ttl_ms,
            check_in.summary,
            check_in.context,
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

fn target_payload(transition: &TargetTransitionRecord<'_>) -> serde_json::Value {
    json!({
        "event": event_name_for(transition.state),
        "target": {
            "name": transition.name,
            "service": transition.service,
            "url": transition.url,
        },
        "state": transition.state,
        "previous_state": transition.previous_state,
        "consecutive_failures": transition.consecutive_failures,
        "last_success_at": if transition.check_succeeded { Some(transition.now) } else { None },
        "sequence": transition.sequence,
        "timestamp": transition.now,
    })
}

fn monitor_payload(transition: &MonitorTransitionRecord<'_>) -> serde_json::Value {
    json!({
        "event": event_name_for(transition.state),
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
        "sequence": transition.sequence,
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
