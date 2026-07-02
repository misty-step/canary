//! Incident correlation persistence.
//!
//! Canary keeps incident correlation deterministic: a signal either attaches to
//! one open incident for the service, updates that incident, resolves it, or is
//! ignored because it is no longer active. This module owns the SQLite
//! transaction so callers do not spread incident invariants through HTTP or
//! worker code.

use canary_core::{
    health::state_machine::HealthState,
    ids::{EventId, IncidentId},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::Result;

const ACTIVE_WINDOW_SECONDS: i64 = 300;
const OPEN_STATE: &str = "investigating";
const RESOLVED_STATE: &str = "resolved";
const INCIDENT_EVENT_SCHEMA_VERSION: &str = "canary.incident_event.v1";

/// One incident-correlation command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncidentCorrelation {
    /// Tenant namespace for the signal.
    pub tenant_id: String,
    /// Project namespace for the signal.
    pub project_id: String,
    /// Signal type, currently `error_group` or `health_transition`.
    pub signal_type: String,
    /// Stable signal reference.
    pub signal_ref: String,
    /// Service name.
    pub service: String,
    /// Incident id to use if this command opens a new incident.
    pub incident_id: IncidentId,
    /// Service-event id to use if this command emits an incident event.
    pub event_id: EventId,
    /// RFC3339 correlation timestamp.
    pub now: String,
}

/// Service event emitted by incident correlation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncidentCorrelationEvent {
    /// Event type.
    pub event: String,
    /// Event id.
    pub id: String,
    /// JSON payload sent to responders.
    pub payload_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IncidentRow {
    id: String,
    tenant_id: String,
    project_id: String,
    service: String,
    state: String,
    severity: String,
    title: Option<String>,
    opened_at: String,
    resolved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SignalRow {
    id: i64,
    signal_type: String,
    signal_ref: String,
    attached_at: String,
    resolved_at: Option<String>,
}

pub(crate) fn correlate(
    connection: &mut Connection,
    command: IncidentCorrelation,
) -> Result<Option<IncidentCorrelationEvent>> {
    let transaction = connection.transaction()?;
    let event = correlate_in_transaction(&transaction, command)?;
    transaction.commit()?;
    Ok(event)
}

pub(crate) fn correlate_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    command: IncidentCorrelation,
) -> Result<Option<IncidentCorrelationEvent>> {
    let signal_active = signal_active(
        transaction,
        &command.signal_type,
        &command.signal_ref,
        &command.tenant_id,
        &command.project_id,
        &command.now,
    )?;
    match open_incident(
        transaction,
        &command.tenant_id,
        &command.project_id,
        &command.service,
    )? {
        None if !signal_active => Ok(None),
        None => create_incident(transaction, &command).map(Some),
        Some(incident) => update_incident(transaction, &incident, &command, signal_active),
    }
}

fn create_incident(
    transaction: &rusqlite::Transaction<'_>,
    command: &IncidentCorrelation,
) -> Result<IncidentCorrelationEvent> {
    let incident_id = command.incident_id.as_str();
    let title = title_for(&command.service);
    transaction.execute(
        "INSERT INTO incidents (
            id, tenant_id, project_id, service, state, severity, title, opened_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'medium', ?6, ?7)",
        params![
            incident_id,
            command.tenant_id,
            command.project_id,
            command.service,
            OPEN_STATE,
            title,
            command.now
        ],
    )?;
    insert_signal(
        transaction,
        incident_id,
        &command.signal_type,
        &command.signal_ref,
        &command.now,
    )?;
    let incident =
        incident_by_id(transaction, incident_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    insert_incident_event(transaction, command, "incident.opened", &incident)
}

fn update_incident(
    transaction: &rusqlite::Transaction<'_>,
    incident: &IncidentRow,
    command: &IncidentCorrelation,
    signal_active: bool,
) -> Result<Option<IncidentCorrelationEvent>> {
    let (signal_changed, attached) =
        sync_signal(transaction, &incident.id, command, signal_active)?;
    let normalized = normalize_signals(
        transaction,
        &incident.id,
        &incident.tenant_id,
        &incident.project_id,
        &command.now,
    )?;
    let mut incident =
        incident_by_id(transaction, &incident.id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let signals = signals_for_incident(transaction, &incident.id)?;
    let incident_changed =
        update_incident_state(transaction, &mut incident, &signals, &command.now)?;

    let event =
        if incident.state == RESOLVED_STATE && (signal_changed || normalized || incident_changed) {
            Some("incident.resolved")
        } else if attached || signal_changed || normalized || incident_changed {
            Some("incident.updated")
        } else {
            None
        };

    match event {
        Some(event) => insert_incident_event(transaction, command, event, &incident).map(Some),
        None => Ok(None),
    }
}

fn sync_signal(
    transaction: &rusqlite::Transaction<'_>,
    incident_id: &str,
    command: &IncidentCorrelation,
    signal_active: bool,
) -> Result<(bool, bool)> {
    let signal = signal_by_ref(
        transaction,
        incident_id,
        &command.signal_type,
        &command.signal_ref,
    )?;
    match (signal, signal_active) {
        (None, true) => {
            insert_signal(
                transaction,
                incident_id,
                &command.signal_type,
                &command.signal_ref,
                &command.now,
            )?;
            Ok((true, true))
        }
        (None, false) => Ok((false, false)),
        (Some(signal), true) if signal.resolved_at.is_some() => {
            transaction.execute(
                "UPDATE incident_signals
                 SET attached_at = ?1, resolved_at = NULL
                 WHERE id = ?2",
                params![command.now, signal.id],
            )?;
            Ok((true, false))
        }
        (Some(signal), false) if signal.resolved_at.is_none() => {
            transaction.execute(
                "UPDATE incident_signals
                 SET resolved_at = ?1
                 WHERE id = ?2",
                params![command.now, signal.id],
            )?;
            Ok((true, false))
        }
        _ => Ok((false, false)),
    }
}

fn normalize_signals(
    transaction: &rusqlite::Transaction<'_>,
    incident_id: &str,
    tenant_id: &str,
    project_id: &str,
    now: &str,
) -> Result<bool> {
    let mut changed = false;
    for signal in signals_for_incident(transaction, incident_id)? {
        let active = signal_active(
            transaction,
            &signal.signal_type,
            &signal.signal_ref,
            tenant_id,
            project_id,
            now,
        )?;
        if active && signal.resolved_at.is_some() {
            transaction.execute(
                "UPDATE incident_signals
                 SET attached_at = ?1, resolved_at = NULL
                 WHERE id = ?2",
                params![now, signal.id],
            )?;
            changed = true;
        } else if !active && signal.resolved_at.is_none() {
            transaction.execute(
                "UPDATE incident_signals
                 SET resolved_at = ?1
                 WHERE id = ?2",
                params![now, signal.id],
            )?;
            changed = true;
        }
    }
    Ok(changed)
}

fn update_incident_state(
    transaction: &rusqlite::Transaction<'_>,
    incident: &mut IncidentRow,
    signals: &[SignalRow],
    now: &str,
) -> Result<bool> {
    let active_signals = signals
        .iter()
        .filter(|signal| signal.resolved_at.is_none())
        .collect::<Vec<_>>();
    let severity = desired_severity(&active_signals, now);
    let (state, resolved_at) = if active_signals.is_empty() {
        (RESOLVED_STATE.to_owned(), Some(now.to_owned()))
    } else {
        (OPEN_STATE.to_owned(), None)
    };

    if incident.state == state
        && incident.severity == severity
        && incident.resolved_at == resolved_at
    {
        return Ok(false);
    }

    transaction.execute(
        "UPDATE incidents
         SET state = ?1, severity = ?2, resolved_at = ?3
         WHERE id = ?4",
        params![state, severity, resolved_at, incident.id],
    )?;
    incident.state = state;
    incident.severity = severity;
    incident.resolved_at = resolved_at;
    Ok(true)
}

fn desired_severity(active_signals: &[&SignalRow], now: &str) -> String {
    let recent = active_signals
        .iter()
        .filter(|signal| signal_counts_for_severity(signal, now))
        .count();
    if recent >= 3 { "high" } else { "medium" }.to_owned()
}

fn signal_counts_for_severity(signal: &SignalRow, now: &str) -> bool {
    // Intentional divergence: health-transition signals are active state, not attached_at recency.
    signal.signal_type == "health_transition" || within_active_window(&signal.attached_at, now)
}

fn signal_active(
    transaction: &rusqlite::Transaction<'_>,
    signal_type: &str,
    signal_ref: &str,
    tenant_id: &str,
    project_id: &str,
    now: &str,
) -> Result<bool> {
    match signal_type {
        "error_group" => {
            let row = transaction
                .query_row(
                    "SELECT status, last_seen_at
                     FROM error_groups
                     WHERE tenant_id = ?1 AND project_id = ?2 AND group_hash = ?3",
                    params![tenant_id, project_id, signal_ref],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            Ok(row.is_some_and(|(status, last_seen_at)| {
                status == "active" && within_active_window(&last_seen_at, now)
            }))
        }
        "health_transition" => {
            let target = state_by_ref(transaction, "target_state", "target_id", signal_ref)?;
            let monitor = state_by_ref(transaction, "monitor_state", "monitor_id", signal_ref)?;
            Ok(target
                .or(monitor)
                .is_some_and(|state| HealthState::persisted_incident_signal_active(&state)))
        }
        _ => Ok(false),
    }
}

fn state_by_ref(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    id_column: &str,
    id: &str,
) -> Result<Option<String>> {
    let sql = format!("SELECT state FROM {table} WHERE {id_column} = ?1");
    Ok(transaction
        .query_row(&sql, [id], |row| row.get::<_, String>(0))
        .optional()?)
}

fn open_incident(
    transaction: &rusqlite::Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    service: &str,
) -> Result<Option<IncidentRow>> {
    transaction
        .query_row(
            "SELECT id, tenant_id, project_id, service, state, severity, title, opened_at, resolved_at
             FROM incidents
             WHERE tenant_id = ?1 AND project_id = ?2 AND service = ?3 AND state != ?4
             ORDER BY opened_at DESC
             LIMIT 1",
            params![tenant_id, project_id, service, RESOLVED_STATE],
            incident_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn incident_by_id(
    transaction: &rusqlite::Transaction<'_>,
    incident_id: &str,
) -> Result<Option<IncidentRow>> {
    transaction
        .query_row(
            "SELECT id, tenant_id, project_id, service, state, severity, title, opened_at, resolved_at
             FROM incidents
             WHERE id = ?1",
            [incident_id],
            incident_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn incident_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IncidentRow> {
    Ok(IncidentRow {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        project_id: row.get(2)?,
        service: row.get(3)?,
        state: row.get(4)?,
        severity: row.get(5)?,
        title: row.get(6)?,
        opened_at: row.get(7)?,
        resolved_at: row.get(8)?,
    })
}

fn signal_by_ref(
    transaction: &rusqlite::Transaction<'_>,
    incident_id: &str,
    signal_type: &str,
    signal_ref: &str,
) -> Result<Option<SignalRow>> {
    transaction
        .query_row(
            "SELECT id, signal_type, signal_ref, attached_at, resolved_at
             FROM incident_signals
             WHERE incident_id = ?1 AND signal_type = ?2 AND signal_ref = ?3",
            params![incident_id, signal_type, signal_ref],
            signal_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn signals_for_incident(
    transaction: &rusqlite::Transaction<'_>,
    incident_id: &str,
) -> Result<Vec<SignalRow>> {
    let mut statement = transaction.prepare(
        "SELECT id, signal_type, signal_ref, attached_at, resolved_at
         FROM incident_signals
         WHERE incident_id = ?1
         ORDER BY attached_at ASC, id ASC",
    )?;
    let signals = statement
        .query_map([incident_id], signal_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(signals)
}

fn signal_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SignalRow> {
    Ok(SignalRow {
        id: row.get(0)?,
        signal_type: row.get(1)?,
        signal_ref: row.get(2)?,
        attached_at: row.get(3)?,
        resolved_at: row.get(4)?,
    })
}

fn insert_signal(
    transaction: &rusqlite::Transaction<'_>,
    incident_id: &str,
    signal_type: &str,
    signal_ref: &str,
    now: &str,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO incident_signals (incident_id, signal_type, signal_ref, attached_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![incident_id, signal_type, signal_ref, now],
    )?;
    Ok(())
}

fn insert_incident_event(
    transaction: &rusqlite::Transaction<'_>,
    command: &IncidentCorrelation,
    event: &str,
    incident: &IncidentRow,
) -> Result<IncidentCorrelationEvent> {
    let signals = signals_for_incident(transaction, &incident.id)?;
    let payload_json = incident_payload(event, incident, &signals, &command.now).to_string();
    let summary = match event {
        "incident.opened" => format!("{}: incident opened", incident.service),
        "incident.updated" => format!("{}: incident updated", incident.service),
        "incident.resolved" => format!("{}: incident resolved", incident.service),
        _ => format!("{}: {event}", incident.service),
    };
    transaction.execute(
        "INSERT INTO service_events (
            id, tenant_id, project_id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'incident', ?6, ?7, ?8, ?9, ?10)",
        params![
            command.event_id.as_str(),
            incident.tenant_id,
            incident.project_id,
            incident.service,
            event,
            incident.id,
            incident.severity,
            summary,
            payload_json,
            command.now,
        ],
    )?;
    Ok(IncidentCorrelationEvent {
        event: event.to_owned(),
        id: command.event_id.to_string(),
        payload_json,
    })
}

fn incident_payload(
    event: &str,
    incident: &IncidentRow,
    signals: &[SignalRow],
    now: &str,
) -> serde_json::Value {
    json!({
        "schema_version": INCIDENT_EVENT_SCHEMA_VERSION,
        "event": event,
        "tenant_id": incident.tenant_id,
        "project_id": incident.project_id,
        "subject": {
            "type": "incident",
            "id": incident.id,
            "service": incident.service,
        },
        "signal": incident_signal_payload(incident, signals, now),
        "replay": {
            "timeline_url": format!("/api/v1/timeline?service={}&window=1h", incident.service),
            "report_url": "/api/v1/report?window=1h",
            "incident_url": format!("/api/v1/incidents/{}", incident.id),
        },
        "incident": {
            "id": incident.id,
            "service": incident.service,
            "state": incident.state,
            "severity": incident.severity,
            "title": incident.title,
            "opened_at": incident.opened_at,
            "resolved_at": incident.resolved_at,
            "signals": signals.iter().map(|signal| {
                json!({
                    "signal_type": signal.signal_type,
                    "signal_ref": signal.signal_ref,
                    "attached_at": signal.attached_at,
                    "resolved_at": signal.resolved_at,
                })
            }).collect::<Vec<_>>(),
        },
        "timestamp": now,
    })
}

fn incident_signal_payload(
    incident: &IncidentRow,
    signals: &[SignalRow],
    now: &str,
) -> serde_json::Value {
    match signals.first() {
        Some(signal) => json!({
            "kind": signal.signal_type,
            "fingerprint": signal.signal_ref,
            "severity": incident.severity,
            "observed_at": signal.attached_at,
        }),
        None => json!({
            "kind": "incident",
            "fingerprint": incident.id,
            "severity": incident.severity,
            "observed_at": now,
        }),
    }
}

fn title_for(service: &str) -> String {
    format!("{service} incident")
}

fn within_active_window(timestamp: &str, now: &str) -> bool {
    let Ok(timestamp) = OffsetDateTime::parse(timestamp, &Rfc3339) else {
        return false;
    };
    let Ok(now) = OffsetDateTime::parse(now, &Rfc3339) else {
        return false;
    };
    (now - timestamp).whole_seconds() <= ACTIVE_WINDOW_SECONDS
}
