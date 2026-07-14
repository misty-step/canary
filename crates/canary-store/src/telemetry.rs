//! Bounded consumer telemetry events backed by the service timeline.

use canary_core::{
    ids::{EventId, IncidentId},
    query::{OperationalSignalContext, TelemetryEvent},
};
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
    /// Optional bounded operational signal that participates in incident correlation.
    pub operational: Option<OperationalSignalInsert>,
}

/// Bounded caller-defined operational signal carried by one telemetry event.
#[derive(Debug, Clone)]
pub struct OperationalSignalInsert {
    /// Stable caller-defined subject type.
    pub subject_type: String,
    /// Stable caller-defined subject id.
    pub subject_id: String,
    /// Producer-declared current state (`active` or `resolved`).
    pub state: String,
    /// Responsible owner.
    pub owner: String,
    /// Link to evidence retained outside Canary.
    pub evidence_url: String,
    /// Producer observation clock.
    pub observed_at: String,
    /// Incident id reserved for a possible open.
    pub incident_id: IncidentId,
    /// Incident event id reserved for a possible open or update.
    pub incident_event_id: EventId,
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
    /// Incident correlation rejected the atomic write.
    #[error("store error: {0}")]
    Store(#[from] crate::StoreError),
}

/// Result returned by telemetry persistence.
pub type TelemetryEventResult<T> = std::result::Result<T, TelemetryEventError>;

/// Atomically persisted telemetry row and any incident responder event it emitted.
#[derive(Debug, Clone)]
pub struct TelemetryEventCommit {
    /// Stored event receipt.
    pub event: TelemetryEvent,
    /// Incident webhook event produced by correlation, when state changed or refreshed.
    pub incident_event: Option<crate::IncidentCorrelationEvent>,
}

pub(crate) fn insert_event(
    connection: &mut Connection,
    event: TelemetryEventInsert,
) -> TelemetryEventResult<TelemetryEventCommit> {
    validate(&event)?;
    let attributes = decode_attributes(&event.attributes_json)?;
    let operational = event
        .operational
        .as_ref()
        .map(|signal| OperationalSignalContext {
            name: event.name.clone(),
            subject_type: signal.subject_type.clone(),
            subject_id: signal.subject_id.clone(),
            state: signal.state.clone(),
            owner: signal.owner.clone(),
            evidence_url: signal.evidence_url.clone(),
            observed_at: signal.observed_at.clone(),
            received_at: event.created_at.clone(),
        });
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
        "operational": operational,
    });
    let payload_json = payload.to_string();

    let transaction = connection.transaction()?;
    let (entity_type, entity_ref, signal_kind) = match event.operational.as_ref() {
        Some(signal) => (
            "operational_signal",
            operational_signal_ref(&signal.subject_type, &signal.subject_id),
            "operational",
        ),
        None => ("telemetry_event", event.id.to_string(), "analytics_event"),
    };

    transaction.execute(
        "INSERT INTO service_events (
            id, tenant_id, project_id, service, event, signal_kind, signal_name,
            entity_type, entity_ref, severity, summary, attributes, retention_class,
            privacy_policy, sampling_policy, payload, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                   ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            event.id.as_str(),
            event.tenant_id,
            event.project_id,
            event.service,
            TELEMETRY_EVENT,
            signal_kind,
            event.name,
            entity_type,
            entity_ref,
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

    let incident_event = match event.operational.as_ref() {
        Some(signal) => crate::incidents::correlate_in_transaction(
            &transaction,
            crate::IncidentCorrelation {
                tenant_id: event.tenant_id.clone(),
                project_id: event.project_id.clone(),
                signal_type: "operational_event".to_owned(),
                signal_ref: operational_signal_ref(&signal.subject_type, &signal.subject_id),
                service: event.service.clone(),
                incident_id: signal.incident_id.clone(),
                event_id: signal.incident_event_id.clone(),
                now: event.created_at.clone(),
            },
        )?,
        None => None,
    };
    let incident_event_name = incident_event
        .as_ref()
        .map(|incident| incident.event.clone());
    let incident_id = incident_event
        .as_ref()
        .map(|incident| incident.incident_id.clone());
    transaction.commit()?;

    Ok(TelemetryEventCommit {
        event: TelemetryEvent {
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
            operational,
            incident_event: incident_event_name,
            incident_id,
        },
        incident_event,
    })
}

pub(crate) fn operational_signal_ref(subject_type: &str, subject_id: &str) -> String {
    format!("{subject_type}/{subject_id}")
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
    if let Some(signal) = event.operational.as_ref() {
        validate_operational_signal(signal, &event.attributes_json, &mut errors);
        if event.retention_class != "audit" {
            errors.push(("retention_class", "must be audit for operational signals"));
        }
        if event.privacy_policy != "redacted" {
            errors.push(("privacy_policy", "must be redacted for operational signals"));
        }
        if event.sampling_policy != "unsampled" {
            errors.push((
                "sampling_policy",
                "must be unsampled for operational signals",
            ));
        }
    }
    if !errors.is_empty() {
        return Err(TelemetryEventError::Validation(errors));
    }
    let _ = decode_attributes(&event.attributes_json)?;
    Ok(())
}

fn validate_operational_signal(
    signal: &OperationalSignalInsert,
    attributes_json: &str,
    errors: &mut Vec<(&'static str, &'static str)>,
) {
    if !bounded_subject_component(&signal.subject_type, 64)
        || signal
            .subject_type
            .chars()
            .any(|character| character.is_ascii_uppercase())
    {
        errors.push((
            "operational.subject.type",
            "must use 1-64 lowercase letters, digits, dots, underscores, or hyphens",
        ));
    }
    if !bounded_subject_component(&signal.subject_id, 160) {
        errors.push((
            "operational.subject.id",
            "must use 1-160 letters, digits, dots, underscores, colons, or hyphens",
        ));
    }
    if !matches!(signal.state.as_str(), "active" | "resolved") {
        errors.push(("operational.state", "must be one of: active, resolved"));
    }
    if signal.owner.trim().is_empty() || signal.owner.chars().count() > 128 {
        errors.push((
            "operational.owner",
            "must be a non-empty string no longer than 128 characters",
        ));
    }
    let valid_evidence_url = url::Url::parse(&signal.evidence_url).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    });
    if signal.evidence_url.chars().count() > 2048 || !valid_evidence_url {
        errors.push((
            "operational.evidence_url",
            "must be an https URL no longer than 2048 characters",
        ));
    }
    if time::OffsetDateTime::parse(
        &signal.observed_at,
        &time::format_description::well_known::Rfc3339,
    )
    .is_err()
    {
        errors.push(("operational.observed_at", "must be an RFC3339 timestamp"));
    }
    if attributes_json != "{}" {
        errors.push((
            "attributes",
            "must be empty for operational signals; retain metrics and snapshots at the evidence URL",
        ));
    }
}

fn bounded_subject_component(value: &str, max: usize) -> bool {
    let len = value.chars().count();
    len > 0
        && len <= max
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | ':' | '-')
        })
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
