//! Transactional persistence for error ingest.
//!
//! Validation and grouping policy live outside this module. This module owns the
//! database invariant that an ingested error, its group mutation, and the
//! service-event ledger append commit together.

use canary_core::{
    ids::{ErrorId, EventId},
    ingest::classification::{Category, Classification, Component, Persistence},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::Result;

/// IDs supplied by the caller for one ingest transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorIngestIds {
    /// Error row id.
    pub error_id: ErrorId,
    /// Service-event row id used when the ingest emits a timeline event.
    pub event_id: EventId,
}

/// Already-normalized payload fields for one error row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorIngestPayload {
    /// Service name.
    pub service: String,
    /// Error class.
    pub error_class: String,
    /// Message truncated to the Phoenix limit by the caller.
    pub message: String,
    /// Normalized message template.
    pub message_template: String,
    /// Optional stack trace truncated to the Phoenix limit by the caller.
    pub stack_trace: Option<String>,
    /// Optional JSON-encoded context.
    pub context_json: Option<String>,
    /// Severity, defaulting to `error` before reaching the store.
    pub severity: String,
    /// Environment, defaulting to `production` before reaching the store.
    pub environment: String,
    /// Stable group hash.
    pub group_hash: String,
    /// Optional JSON-encoded fingerprint.
    pub fingerprint_json: Option<String>,
    /// Optional region.
    pub region: Option<String>,
    /// Deterministic classification.
    pub classification: Classification,
    /// RFC3339 timestamp string.
    pub created_at: String,
}

/// One validated ingest transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorIngest {
    /// Row IDs for the transaction.
    pub ids: ErrorIngestIds,
    /// Error payload.
    pub payload: ErrorIngestPayload,
}

/// Service event appended by the transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorServiceEvent {
    /// Event type.
    pub event: String,
    /// Event id.
    pub id: String,
    /// JSON payload sent to responders.
    pub payload_json: String,
}

/// Summary returned to the ingest caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorIngestCommit {
    /// Error row id.
    pub id: String,
    /// Stable group hash.
    pub group_hash: String,
    /// Whether this was a newly-created error class.
    pub is_new_class: bool,
    /// Timeline event appended in the same transaction, if any.
    pub service_event: Option<ErrorServiceEvent>,
}

pub(crate) fn commit(
    connection: &mut Connection,
    ingest: ErrorIngest,
) -> Result<ErrorIngestCommit> {
    let transaction = connection.transaction()?;
    insert_error(&transaction, &ingest)?;

    let group = load_group(&transaction, &ingest.payload.group_hash)?;
    let (is_new_class, event) = match group {
        None => {
            insert_group(&transaction, &ingest)?;
            (true, Some("error.new_class"))
        }
        Some(group) => {
            update_group(&transaction, &ingest, group.total_count)?;
            (
                false,
                regression_event(&group.last_seen_at, &ingest.payload.created_at),
            )
        }
    };

    let service_event = match event {
        Some(event) => Some(insert_service_event(&transaction, &ingest, event)?),
        None => None,
    };

    transaction.commit()?;

    Ok(ErrorIngestCommit {
        id: ingest.ids.error_id.into_string(),
        group_hash: ingest.payload.group_hash,
        is_new_class,
        service_event,
    })
}

fn insert_error(transaction: &rusqlite::Transaction<'_>, ingest: &ErrorIngest) -> Result<()> {
    let classification = ingest.payload.classification;
    transaction.execute(
        "INSERT INTO errors (
            id, service, error_class, message, message_template, stack_trace, context,
            severity, environment, group_hash, fingerprint, region,
            classification_category, classification_persistence, classification_component,
            created_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
         )",
        params![
            ingest.ids.error_id.as_str(),
            ingest.payload.service,
            ingest.payload.error_class,
            ingest.payload.message,
            ingest.payload.message_template,
            ingest.payload.stack_trace,
            ingest.payload.context_json,
            ingest.payload.severity,
            ingest.payload.environment,
            ingest.payload.group_hash,
            ingest.payload.fingerprint_json,
            ingest.payload.region,
            category_str(classification.category),
            persistence_str(classification.persistence),
            component_str(classification.component),
            ingest.payload.created_at,
        ],
    )?;
    Ok(())
}

#[derive(Debug)]
struct ExistingGroup {
    last_seen_at: String,
    total_count: i64,
}

fn load_group(
    transaction: &rusqlite::Transaction<'_>,
    group_hash: &str,
) -> Result<Option<ExistingGroup>> {
    let group = transaction
        .query_row(
            "SELECT last_seen_at, total_count FROM error_groups WHERE group_hash = ?1",
            [group_hash],
            |row| {
                Ok(ExistingGroup {
                    last_seen_at: row.get(0)?,
                    total_count: row.get(1)?,
                })
            },
        )
        .optional()?;
    Ok(group)
}

fn insert_group(transaction: &rusqlite::Transaction<'_>, ingest: &ErrorIngest) -> Result<()> {
    transaction.execute(
        "INSERT INTO error_groups (
            group_hash, service, error_class, message_template, severity,
            first_seen_at, last_seen_at, total_count, last_error_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8)",
        params![
            ingest.payload.group_hash,
            ingest.payload.service,
            ingest.payload.error_class,
            ingest.payload.message_template,
            ingest.payload.severity,
            ingest.payload.created_at,
            ingest.payload.created_at,
            ingest.ids.error_id.as_str(),
        ],
    )?;
    Ok(())
}

fn update_group(
    transaction: &rusqlite::Transaction<'_>,
    ingest: &ErrorIngest,
    total_count: i64,
) -> Result<()> {
    transaction.execute(
        "UPDATE error_groups
         SET last_seen_at = ?1, total_count = ?2, last_error_id = ?3, status = 'active'
         WHERE group_hash = ?4",
        params![
            ingest.payload.created_at,
            total_count + 1,
            ingest.ids.error_id.as_str(),
            ingest.payload.group_hash,
        ],
    )?;
    Ok(())
}

fn regression_event(last_seen_at: &str, now: &str) -> Option<&'static str> {
    let last = OffsetDateTime::parse(last_seen_at, &Rfc3339).ok()?;
    let now = OffsetDateTime::parse(now, &Rfc3339).ok()?;
    (now - last)
        .whole_hours()
        .ge(&24)
        .then_some("error.regression")
}

fn insert_service_event(
    transaction: &rusqlite::Transaction<'_>,
    ingest: &ErrorIngest,
    event: &str,
) -> Result<ErrorServiceEvent> {
    let payload = json!({
        "event": event,
        "error": {
            "id": ingest.ids.error_id.as_str(),
            "service": ingest.payload.service,
            "error_class": ingest.payload.error_class,
            "message": ingest.payload.message,
            "severity": ingest.payload.severity,
            "group_hash": ingest.payload.group_hash,
        },
        "timestamp": ingest.payload.created_at,
    });
    let payload_json = payload.to_string();
    let summary = match event {
        "error.new_class" => format!(
            "{}: new {}",
            ingest.payload.service, ingest.payload.error_class
        ),
        "error.regression" => format!(
            "{}: {} regressed",
            ingest.payload.service, ingest.payload.error_class
        ),
        _ => format!("{}: {event}", ingest.payload.service),
    };

    transaction.execute(
        "INSERT INTO service_events (
            id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
         ) VALUES (?1, ?2, ?3, 'error_group', ?4, ?5, ?6, ?7, ?8)",
        params![
            ingest.ids.event_id.as_str(),
            ingest.payload.service,
            event,
            ingest.payload.group_hash,
            ingest.payload.severity,
            summary,
            payload_json,
            ingest.payload.created_at,
        ],
    )?;

    Ok(ErrorServiceEvent {
        event: event.to_owned(),
        id: ingest.ids.event_id.to_string(),
        payload_json,
    })
}

const fn category_str(category: Category) -> &'static str {
    match category {
        Category::Infrastructure => "infrastructure",
        Category::Application => "application",
        Category::Unknown => "unknown",
    }
}

const fn persistence_str(persistence: Persistence) -> &'static str {
    match persistence {
        Persistence::Transient => "transient",
        Persistence::Persistent => "persistent",
        Persistence::Unknown => "unknown",
    }
}

const fn component_str(component: Component) -> &'static str {
    match component {
        Component::Database => "database",
        Component::Network => "network",
        Component::Runtime => "runtime",
        Component::Unknown => "unknown",
    }
}
