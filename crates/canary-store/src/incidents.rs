//! Incident correlation persistence.
//!
//! Canary keeps incident correlation deterministic: a signal either attaches to
//! one open incident for the service, updates that incident, resolves it, or is
//! ignored because it is no longer active. This module owns the SQLite
//! transaction so callers do not spread incident invariants through HTTP or
//! worker code.

use canary_core::ids::{EventId, IncidentId};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::Result;

const ACTIVE_WINDOW_SECONDS: i64 = 300;
const OPEN_STATE: &str = "investigating";
const RESOLVED_STATE: &str = "resolved";

/// One incident-correlation command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncidentCorrelation {
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
    let signal_active = signal_active(
        &transaction,
        &command.signal_type,
        &command.signal_ref,
        &command.now,
    )?;
    let event = match open_incident(&transaction, &command.service)? {
        None if !signal_active => None,
        None => Some(create_incident(&transaction, &command)?),
        Some(incident) => update_incident(&transaction, &incident, &command, signal_active)?,
    };
    transaction.commit()?;
    Ok(event)
}

fn create_incident(
    transaction: &rusqlite::Transaction<'_>,
    command: &IncidentCorrelation,
) -> Result<IncidentCorrelationEvent> {
    let incident_id = command.incident_id.as_str();
    let title = title_for(&command.service);
    transaction.execute(
        "INSERT INTO incidents (id, service, state, severity, title, opened_at)
         VALUES (?1, ?2, ?3, 'medium', ?4, ?5)",
        params![incident_id, command.service, OPEN_STATE, title, command.now],
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
    let normalized = normalize_signals(transaction, &incident.id, &command.now)?;
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
    now: &str,
) -> Result<bool> {
    let mut changed = false;
    for signal in signals_for_incident(transaction, incident_id)? {
        let active = signal_active(transaction, &signal.signal_type, &signal.signal_ref, now)?;
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
        .filter(|signal| within_active_window(&signal.attached_at, now))
        .count();
    if recent >= 3 { "high" } else { "medium" }.to_owned()
}

fn signal_active(
    transaction: &rusqlite::Transaction<'_>,
    signal_type: &str,
    signal_ref: &str,
    now: &str,
) -> Result<bool> {
    match signal_type {
        "error_group" => {
            let row = transaction
                .query_row(
                    "SELECT status, last_seen_at FROM error_groups WHERE group_hash = ?1",
                    [signal_ref],
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
            Ok(target.or(monitor).is_some_and(|state| state != "up"))
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
    service: &str,
) -> Result<Option<IncidentRow>> {
    transaction
        .query_row(
            "SELECT id, service, state, severity, title, opened_at, resolved_at
             FROM incidents
             WHERE service = ?1 AND state != ?2
             ORDER BY opened_at DESC
             LIMIT 1",
            params![service, RESOLVED_STATE],
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
            "SELECT id, service, state, severity, title, opened_at, resolved_at
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
        service: row.get(1)?,
        state: row.get(2)?,
        severity: row.get(3)?,
        title: row.get(4)?,
        opened_at: row.get(5)?,
        resolved_at: row.get(6)?,
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
            id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
         ) VALUES (?1, ?2, ?3, 'incident', ?4, ?5, ?6, ?7, ?8)",
        params![
            command.event_id.as_str(),
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
        "event": event,
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
