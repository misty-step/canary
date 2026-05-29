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
    /// Persisted target-state sequence after the probe.
    pub sequence: i64,
}

/// Persisted outcome of one monitor check-in observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorCheckInCommitResult {
    /// Health transition emitted by this check-in, when it changed state.
    pub transition: Option<HealthTransitionCommit>,
    /// Persisted monitor-state sequence after the check-in.
    pub sequence: i64,
}

/// Persisted outcome of one monitor overdue evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorOverdueCommitResult {
    /// Health transition emitted by this overdue evaluation.
    pub transition: HealthTransitionCommit,
    /// Persisted monitor-state sequence after the overdue evaluation.
    pub sequence: i64,
}

/// Monitor row to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInsert {
    /// Monitor id.
    pub id: String,
    /// Unique monitor name used by check-in clients.
    pub name: String,
    /// Service name resolved with Phoenix's monitor service fallback.
    pub service: String,
    /// Monitor mode, `schedule` or `ttl`.
    pub mode: String,
    /// Expected interval in milliseconds.
    pub expected_every_ms: i64,
    /// Grace period in milliseconds.
    pub grace_ms: i64,
    /// Creation timestamp.
    pub created_at: String,
}

/// HTTP target row to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInsert {
    /// Target id.
    pub id: String,
    /// Probe URL.
    pub url: String,
    /// Target display name.
    pub name: String,
    /// Service name resolved with Phoenix's target service fallback.
    pub service: String,
    /// HTTP method, currently `GET` or `HEAD`.
    pub method: String,
    /// JSON encoded request headers.
    pub headers: Option<String>,
    /// Probe interval in milliseconds.
    pub interval_ms: i64,
    /// Probe timeout in milliseconds.
    pub timeout_ms: i64,
    /// Expected HTTP status expression.
    pub expected_status: String,
    /// Required response body substring.
    pub body_contains: Option<String>,
    /// Consecutive failures before degraded.
    pub degraded_after: u32,
    /// Consecutive failures before down.
    pub down_after: u32,
    /// Consecutive successes before recovery.
    pub up_after: u32,
    /// Active flag.
    pub active: bool,
    /// Creation timestamp.
    pub created_at: String,
}

/// Target configuration and state needed to execute and plan one probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetProbeSnapshot {
    /// Target id.
    pub id: String,
    /// Target display name.
    pub name: String,
    /// Service name.
    pub service: String,
    /// Probe URL.
    pub url: String,
    /// HTTP method.
    pub method: String,
    /// JSON encoded request headers.
    pub headers: Option<String>,
    /// Probe timeout in milliseconds.
    pub timeout_ms: i64,
    /// Expected HTTP status expression.
    pub expected_status: String,
    /// Required response body substring.
    pub body_contains: Option<String>,
    /// Consecutive failures before degraded.
    pub degraded_after: u32,
    /// Consecutive failures before down.
    pub down_after: u32,
    /// Consecutive successes before recovery.
    pub up_after: u32,
    /// Current target health state.
    pub state: String,
    /// Current consecutive failure counter.
    pub consecutive_failures: u32,
    /// Current consecutive success counter.
    pub consecutive_successes: u32,
}

/// Active target row needed by the probe lifecycle adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTargetProbeSchedule {
    /// Target id.
    pub target_id: String,
    /// Probe interval in milliseconds.
    pub interval_ms: i64,
}

/// Monitor configuration and state needed to plan one check-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorCheckInSnapshot {
    /// Monitor id.
    pub id: String,
    /// Unique monitor name.
    pub name: String,
    /// Service name.
    pub service: String,
    /// Monitor mode, `schedule` or `ttl`.
    pub mode: String,
    /// Expected interval in milliseconds.
    pub expected_every_ms: i64,
    /// Grace period in milliseconds.
    pub grace_ms: i64,
    /// Current monitor health state.
    pub state: String,
}

/// Monitor configuration and state needed to evaluate overdue deadlines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorOverdueCandidate {
    /// Monitor row id.
    pub id: String,
    /// Monitor display name.
    pub name: String,
    /// Service name.
    pub service: String,
    /// Monitor mode, `schedule` or `ttl`.
    pub mode: String,
    /// Configured expected interval.
    pub expected_every_ms: i64,
    /// Configured grace period.
    pub grace_ms: i64,
    /// Current monitor health state.
    pub state: String,
    /// Last check-in status, when any.
    pub last_check_in_status: Option<String>,
    /// Last check-in timestamp, when any.
    pub last_check_in_at: Option<String>,
    /// Current deadline timestamp.
    pub deadline_at: Option<String>,
    /// First missed deadline timestamp.
    pub first_missed_at: Option<String>,
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

/// One overdue monitor transition to persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorOverdueCommit {
    /// Monitor id.
    pub monitor_id: String,
    /// Persisted state after overdue evaluation.
    pub state: String,
    /// First missed deadline timestamp after this evaluation.
    pub first_missed_at: Option<String>,
    /// Last check-in timestamp to preserve in transition payloads.
    pub last_check_in_at: Option<String>,
    /// Last check-in status to preserve in transition payloads.
    pub last_check_in_status: Option<String>,
    /// Deadline to preserve in transition payloads and state.
    pub deadline_at: Option<String>,
    /// Timestamp for state and transition writes.
    pub now: String,
    /// Transition metadata emitted by this overdue evaluation.
    pub transition: MonitorTransitionEvent,
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
    Ok(TargetProbeCommitResult {
        transition,
        sequence,
    })
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
            first_missed_at: MonitorFirstMissedAtWrite::Clear,
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
    Ok(MonitorCheckInCommitResult {
        transition,
        sequence,
    })
}

pub(crate) fn commit_monitor_overdue(
    connection: &mut rusqlite::Connection,
    overdue: MonitorOverdueCommit,
) -> Result<MonitorOverdueCommitResult> {
    let transaction = connection.transaction()?;
    let sequence = next_sequence(
        &transaction,
        "monitor_state",
        "monitor_id",
        &overdue.monitor_id,
    )?;
    upsert_monitor_state_fields(
        &transaction,
        MonitorStateWrite {
            monitor_id: &overdue.monitor_id,
            state: &overdue.state,
            last_check_in_at: None,
            last_check_in_status: None,
            deadline_at: overdue.deadline_at.as_deref(),
            first_missed_at: match overdue.first_missed_at.as_deref() {
                Some(first_missed_at) => MonitorFirstMissedAtWrite::Set(first_missed_at),
                None => MonitorFirstMissedAtWrite::Preserve,
            },
            now: &overdue.now,
            transitioned: true,
        },
        sequence,
    )?;
    let transition = record_monitor_transition(
        &transaction,
        MonitorTransitionRecord {
            monitor_id: &overdue.monitor_id,
            name: &overdue.transition.name,
            service: &overdue.transition.service,
            mode: &overdue.transition.mode,
            expected_every_ms: overdue.transition.expected_every_ms,
            grace_ms: overdue.transition.grace_ms,
            previous_state: &overdue.transition.previous_state,
            state: &overdue.state,
            last_check_in_at: overdue.last_check_in_at.as_deref(),
            last_check_in_status: overdue.last_check_in_status.as_deref(),
            deadline_at: overdue.deadline_at.as_deref(),
            now: &overdue.now,
            event_id: &overdue.transition.event_id,
            incident_id: overdue.transition.incident_id,
            incident_event_id: overdue.transition.incident_event_id,
            sequence,
        },
    )?;

    transaction.commit()?;
    Ok(MonitorOverdueCommitResult {
        transition,
        sequence,
    })
}

pub(crate) fn insert_monitor(
    connection: &rusqlite::Connection,
    monitor: MonitorInsert,
) -> Result<()> {
    connection.execute(
        "INSERT INTO monitors (id, name, service, mode, expected_every_ms, grace_ms, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            monitor.id,
            monitor.name,
            monitor.service,
            monitor.mode,
            monitor.expected_every_ms,
            monitor.grace_ms,
            monitor.created_at,
        ],
    )?;
    Ok(())
}

pub(crate) fn insert_target(connection: &rusqlite::Connection, target: TargetInsert) -> Result<()> {
    connection.execute(
        "INSERT INTO targets (
            id, url, name, service, method, headers, interval_ms, timeout_ms,
            expected_status, body_contains, degraded_after, down_after,
            up_after, active, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            target.id,
            target.url,
            target.name,
            target.service,
            target.method,
            target.headers,
            target.interval_ms,
            target.timeout_ms,
            target.expected_status,
            target.body_contains,
            target.degraded_after,
            target.down_after,
            target.up_after,
            if target.active { 1 } else { 0 },
            target.created_at,
        ],
    )?;
    Ok(())
}

pub(crate) fn update_target_active(
    connection: &rusqlite::Connection,
    target_id: &str,
    active: bool,
) -> Result<bool> {
    let changed = connection.execute(
        "UPDATE targets SET active = ?2 WHERE id = ?1",
        rusqlite::params![target_id, if active { 1 } else { 0 }],
    )?;
    Ok(changed > 0)
}

pub(crate) fn target_probe_snapshot_by_id(
    connection: &mut rusqlite::Connection,
    target_id: &str,
) -> Result<Option<TargetProbeSnapshot>> {
    let transaction = connection.transaction()?;
    let target = transaction
        .query_row(
            "SELECT
                id, name, COALESCE(NULLIF(service, ''), name), url,
                COALESCE(method, 'GET'), headers, COALESCE(timeout_ms, 10000),
                COALESCE(expected_status, '200'), body_contains,
                COALESCE(degraded_after, 1), COALESCE(down_after, 3),
                COALESCE(up_after, 1)
             FROM targets WHERE id = ?1 AND active = 1",
            [target_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, u32>(9)?,
                    row.get::<_, u32>(10)?,
                    row.get::<_, u32>(11)?,
                ))
            },
        )
        .optional()?;

    let Some((
        id,
        name,
        service,
        url,
        method,
        headers,
        timeout_ms,
        expected_status,
        body_contains,
        degraded_after,
        down_after,
        up_after,
    )) = target
    else {
        transaction.commit()?;
        return Ok(None);
    };

    transaction.execute(
        "INSERT OR IGNORE INTO target_state (target_id, state) VALUES (?1, 'unknown')",
        [&id],
    )?;
    let (state, consecutive_failures, consecutive_successes) = transaction.query_row(
        "SELECT
            state, COALESCE(consecutive_failures, 0), COALESCE(consecutive_successes, 0)
         FROM target_state WHERE target_id = ?1",
        [&id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
            ))
        },
    )?;
    transaction.commit()?;

    Ok(Some(TargetProbeSnapshot {
        id,
        name,
        service,
        url,
        method,
        headers,
        timeout_ms,
        expected_status,
        body_contains,
        degraded_after,
        down_after,
        up_after,
        state,
        consecutive_failures,
        consecutive_successes,
    }))
}

pub(crate) fn active_target_probe_schedules(
    connection: &rusqlite::Connection,
) -> Result<Vec<ActiveTargetProbeSchedule>> {
    let mut statement = connection.prepare(
        "SELECT id, COALESCE(interval_ms, 60000)
         FROM targets
         WHERE active = 1
         ORDER BY id",
    )?;
    let schedules = statement
        .query_map([], |row| {
            Ok(ActiveTargetProbeSchedule {
                target_id: row.get(0)?,
                interval_ms: row.get(1)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(schedules)
}

pub(crate) fn monitor_check_in_snapshot_by_name(
    connection: &mut rusqlite::Connection,
    name: &str,
) -> Result<Option<MonitorCheckInSnapshot>> {
    let transaction = connection.transaction()?;
    let monitor = transaction
        .query_row(
            "SELECT id, name, service, mode, expected_every_ms, grace_ms
             FROM monitors WHERE name = ?1",
            [name],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;

    let Some((id, name, service, mode, expected_every_ms, grace_ms)) = monitor else {
        transaction.commit()?;
        return Ok(None);
    };

    transaction.execute(
        "INSERT OR IGNORE INTO monitor_state (monitor_id, state) VALUES (?1, 'unknown')",
        [&id],
    )?;
    let state = transaction.query_row(
        "SELECT state FROM monitor_state WHERE monitor_id = ?1",
        [&id],
        |row| row.get::<_, String>(0),
    )?;
    transaction.commit()?;

    Ok(Some(MonitorCheckInSnapshot {
        id,
        name,
        service,
        mode,
        expected_every_ms,
        grace_ms,
        state,
    }))
}

pub(crate) fn monitor_overdue_candidates(
    connection: &rusqlite::Connection,
) -> Result<Vec<MonitorOverdueCandidate>> {
    let mut statement = connection.prepare(
        "SELECT
            m.id, m.name, m.service, m.mode, m.expected_every_ms, m.grace_ms,
            s.state, s.last_check_in_status, s.last_check_in_at, s.deadline_at,
            s.first_missed_at
         FROM monitors m
         JOIN monitor_state s ON s.monitor_id = m.id
         WHERE s.deadline_at IS NOT NULL
         ORDER BY m.id",
    )?;
    let candidates = statement
        .query_map([], |row| {
            Ok(MonitorOverdueCandidate {
                id: row.get(0)?,
                name: row.get(1)?,
                service: row.get(2)?,
                mode: row.get(3)?,
                expected_every_ms: row.get(4)?,
                grace_ms: row.get(5)?,
                state: row.get(6)?,
                last_check_in_status: row.get(7)?,
                last_check_in_at: row.get(8)?,
                deadline_at: row.get(9)?,
                first_missed_at: row.get(10)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(candidates)
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

enum MonitorFirstMissedAtWrite<'a> {
    Clear,
    Preserve,
    Set(&'a str),
}

struct MonitorStateWrite<'a> {
    monitor_id: &'a str,
    state: &'a str,
    last_check_in_at: Option<&'a str>,
    last_check_in_status: Option<&'a str>,
    deadline_at: Option<&'a str>,
    first_missed_at: MonitorFirstMissedAtWrite<'a>,
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
    let (first_missed_mode, first_missed_at) = match write.first_missed_at {
        MonitorFirstMissedAtWrite::Clear => ("clear", None),
        MonitorFirstMissedAtWrite::Preserve => ("preserve", None),
        MonitorFirstMissedAtWrite::Set(first_missed_at) => ("set", Some(first_missed_at)),
    };
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
            first_missed_at = CASE ?11
                WHEN 'clear' THEN NULL
                WHEN 'set' THEN excluded.first_missed_at
                ELSE monitor_state.first_missed_at
            END,
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
            first_missed_mode,
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
