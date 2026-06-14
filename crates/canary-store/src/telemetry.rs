//! Bounded consumer telemetry events backed by the service timeline.

use canary_core::{ids::EventId, query::TelemetryEvent};
use rusqlite::{Connection, params, types::Type};
use serde_json::Value;

const MAX_EVENT_NAME_LEN: usize = 128;
const MAX_SUMMARY_LEN: usize = 512;
const MAX_ATTRIBUTES_BYTES: usize = 8_192;
const TELEMETRY_EVENT: &str = "telemetry.event";

/// Insert payload for one analytics event.
#[derive(Debug, Clone)]
pub struct TelemetryEventInsert {
    /// Event id.
    pub id: EventId,
    /// Tenant namespace.
    pub tenant_id: String,
    /// Project namespace.
    pub project_id: String,
    /// Service name.
    pub service: String,
    /// Consumer event name.
    pub name: String,
    /// Event severity.
    pub severity: String,
    /// Bounded human summary.
    pub summary: String,
    /// JSON object attributes after caller redaction.
    pub attributes_json: String,
    /// Retention class.
    pub retention_class: String,
    /// Privacy policy.
    pub privacy_policy: String,
    /// Sampling policy.
    pub sampling_policy: String,
    /// Creation timestamp.
    pub created_at: String,
}

/// Telemetry persistence failure.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryEventError {
    /// Request failed validation.
    #[error("validation error")]
    Validation(Vec<(&'static str, &'static str)>),
    /// SQLite rejected the write.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Result returned by telemetry persistence.
pub type TelemetryEventResult<T> = std::result::Result<T, TelemetryEventError>;

pub(crate) fn insert_event(
    connection: &Connection,
    event: TelemetryEventInsert,
) -> TelemetryEventResult<TelemetryEvent> {
    validate(&event)?;
    let attributes = decode_attributes(&event.attributes_json)?;
    let payload = serde_json::json!({
        "event": TELEMETRY_EVENT,
        "signal_kind": "analytics_event",
        "name": event.name,
        "service": event.service,
        "severity": event.severity,
        "summary": event.summary,
        "attributes": attributes,
        "retention_class": event.retention_class,
        "privacy_policy": event.privacy_policy,
        "sampling_policy": event.sampling_policy,
    });
    let payload_json = payload.to_string();

    connection.execute(
        "INSERT INTO service_events (
            id, tenant_id, project_id, service, event, signal_kind, signal_name,
            entity_type, entity_ref, severity, summary, attributes, retention_class,
            privacy_policy, sampling_policy, payload, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'analytics_event', ?6, 'telemetry_event', ?1,
                   ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            event.id.as_str(),
            event.tenant_id,
            event.project_id,
            event.service,
            TELEMETRY_EVENT,
            event.name,
            event.severity,
            event.summary,
            event.attributes_json,
            event.retention_class,
            event.privacy_policy,
            event.sampling_policy,
            payload_json,
            event.created_at,
        ],
    )?;

    Ok(TelemetryEvent {
        id: event.id.into_string(),
        service: event.service,
        event: TELEMETRY_EVENT.to_owned(),
        name: event.name,
        severity: event.severity,
        summary: event.summary,
        attributes,
        retention_class: event.retention_class,
        privacy_policy: event.privacy_policy,
        sampling_policy: event.sampling_policy,
        created_at: event.created_at,
    })
}

fn validate(event: &TelemetryEventInsert) -> TelemetryEventResult<()> {
    let mut errors = Vec::new();
    if event.service.trim().is_empty() {
        errors.push(("service", "must be a non-empty string"));
    }
    if event.name.trim().is_empty() || event.name.chars().count() > MAX_EVENT_NAME_LEN {
        errors.push((
            "name",
            "must be a non-empty string no longer than 128 characters",
        ));
    }
    if !matches!(event.severity.as_str(), "info" | "warning" | "error") {
        errors.push(("severity", "must be one of: info, warning, error"));
    }
    if event.summary.trim().is_empty() || event.summary.chars().count() > MAX_SUMMARY_LEN {
        errors.push((
            "summary",
            "must be a non-empty string no longer than 512 characters",
        ));
    }
    if event.attributes_json.len() > MAX_ATTRIBUTES_BYTES {
        errors.push((
            "attributes",
            "must be a JSON object no larger than 8192 bytes",
        ));
    }
    if !matches!(
        event.retention_class.as_str(),
        "ephemeral" | "standard" | "audit"
    ) {
        errors.push((
            "retention_class",
            "must be one of: ephemeral, standard, audit",
        ));
    }
    if !matches!(
        event.privacy_policy.as_str(),
        "redacted" | "public" | "sensitive"
    ) {
        errors.push((
            "privacy_policy",
            "must be one of: redacted, public, sensitive",
        ));
    }
    if event.sampling_policy.trim().is_empty() {
        errors.push(("sampling_policy", "must be a non-empty string"));
    }
    if !errors.is_empty() {
        return Err(TelemetryEventError::Validation(errors));
    }
    let _ = decode_attributes(&event.attributes_json)?;
    Ok(())
}

fn decode_attributes(attributes_json: &str) -> TelemetryEventResult<Value> {
    let value = serde_json::from_str::<Value>(attributes_json).map_err(|err| {
        TelemetryEventError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
            0,
            Type::Text,
            Box::new(err),
        ))
    })?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(TelemetryEventError::Validation(vec![(
            "attributes",
            "must be a JSON object",
        )]))
    }
}
