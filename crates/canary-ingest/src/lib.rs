//! Deep error-ingest boundary for the Rust rewrite.
//!
//! HTTP adapters pass decoded JSON here. This crate owns validation order,
//! truncation, grouping, classification, and the single call into
//! `canary-store`.

use std::collections::BTreeMap;

use canary_core::{
    ids::{ErrorId, EventId},
    ingest::{
        classification::classify,
        grouping::{GroupingInput, compute},
    },
};
use canary_store::{
    ErrorIngest, ErrorIngestCommit, ErrorIngestIds, ErrorIngestPayload, Store, StoreError,
};
use serde_json::{Map, Value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_CONTEXT_SIZE: usize = 8_192;
const MAX_FINGERPRINT_ELEMENTS: usize = 5;
const MAX_FINGERPRINT_ELEMENT_LEN: usize = 256;
const MAX_MESSAGE_LEN: usize = 4_096;
const MAX_STACK_TRACE_LEN: usize = 32_768;

/// Result type returned by the ingest boundary.
pub type Result<T> = std::result::Result<T, IngestError>;

/// Error returned before the store commits anything.
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    /// Request body failed field validation.
    #[error("validation error")]
    Validation(ValidationErrors),
    /// Request body exceeded an ingest size limit.
    #[error("{0}")]
    PayloadTooLarge(String),
    /// Persistence failed after validation passed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Field-level validation errors, matching the Phoenix response shape.
pub type ValidationErrors = BTreeMap<String, Vec<String>>;

/// Runtime grouping configuration hidden from HTTP handlers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestConfig {
    /// In-project module prefixes used by stack grouping.
    pub module_prefixes: Vec<String>,
    /// In-project path prefixes used by stack grouping.
    pub path_prefixes: Vec<String>,
}

/// Deterministic metadata supplied by the server boundary for one ingest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestContext {
    /// Error row ID.
    pub error_id: ErrorId,
    /// Service-event row ID used when a timeline event is emitted.
    pub event_id: EventId,
    /// RFC3339 ingest timestamp.
    pub now: String,
}

impl IngestContext {
    /// Build a context using generated IDs and the current UTC time.
    pub fn now() -> Self {
        let now = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned());

        Self {
            error_id: ErrorId::generate(),
            event_id: EventId::generate(),
            now,
        }
    }
}

/// Successful ingest response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestAccepted {
    /// Error row ID.
    pub id: String,
    /// Stable group hash.
    pub group_hash: String,
    /// Whether this was a newly-created error class.
    pub is_new_class: bool,
    /// Best-effort effects to run after the ingest transaction commits.
    pub post_commit_effects: Vec<IngestEffect>,
}

/// Post-commit effect emitted by a successful ingest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestEffect {
    /// Notify in-process subscribers that a new error row exists.
    BroadcastNewError {
        /// Error row id.
        error_id: String,
        /// Service name.
        service: String,
    },
    /// Attach the error group to the service incident graph.
    CorrelateIncident {
        /// Signal type to correlate.
        signal_type: String,
        /// Stable signal reference.
        signal_ref: String,
        /// Service name.
        service: String,
    },
    /// Enqueue responder webhooks for an already-recorded service event.
    EnqueueWebhook {
        /// Event name.
        event: String,
        /// JSON payload to deliver.
        payload_json: String,
    },
}

/// Ingest one decoded JSON object.
pub fn ingest(
    store: &mut Store,
    attrs: &Map<String, Value>,
    config: &IngestConfig,
    context: IngestContext,
) -> Result<IngestAccepted> {
    let command = prepare(attrs, config, context)?;
    let commit = store.commit_error_ingest(command)?;
    Ok(accepted(commit))
}

fn prepare(
    attrs: &Map<String, Value>,
    config: &IngestConfig,
    context: IngestContext,
) -> Result<ErrorIngest> {
    let service = required_string(attrs, "service")?;
    let error_class = required_string(attrs, "error_class")?;
    let message = required_string(attrs, "message")?;

    validate_context(attrs)?;
    let fingerprint = validate_fingerprint(attrs)?;

    let stack_trace = optional_string(attrs, "stack_trace");
    let grouping = compute(GroupingInput {
        service: &service,
        error_class: &error_class,
        message: &message,
        stack_trace: stack_trace.as_deref(),
        fingerprint: fingerprint.as_deref(),
        module_prefixes: &config.module_prefixes,
        path_prefixes: &config.path_prefixes,
    });
    let classification = classify(&error_class, &message);

    Ok(ErrorIngest {
        ids: ErrorIngestIds {
            error_id: context.error_id,
            event_id: context.event_id,
        },
        payload: ErrorIngestPayload {
            service,
            error_class,
            message: truncate(&message, MAX_MESSAGE_LEN),
            message_template: grouping.message_template,
            stack_trace: stack_trace.map(|value| truncate(&value, MAX_STACK_TRACE_LEN)),
            context_json: context_json(attrs),
            severity: optional_string(attrs, "severity").unwrap_or_else(|| "error".to_owned()),
            environment: optional_string(attrs, "environment")
                .unwrap_or_else(|| "production".to_owned()),
            group_hash: grouping.group_hash,
            fingerprint_json: fingerprint_json(fingerprint.as_deref()),
            region: optional_string(attrs, "region"),
            classification,
            created_at: context.now,
        },
    })
}

fn accepted(commit: ErrorIngestCommit) -> IngestAccepted {
    let mut post_commit_effects = vec![
        IngestEffect::BroadcastNewError {
            error_id: commit.id.clone(),
            service: commit.service.clone(),
        },
        IngestEffect::CorrelateIncident {
            signal_type: "error_group".to_owned(),
            signal_ref: commit.group_hash.clone(),
            service: commit.service.clone(),
        },
    ];

    if let Some(event) = &commit.service_event {
        post_commit_effects.push(IngestEffect::EnqueueWebhook {
            event: event.event.clone(),
            payload_json: event.payload_json.clone(),
        });
    }

    IngestAccepted {
        id: commit.id,
        group_hash: commit.group_hash,
        is_new_class: commit.is_new_class,
        post_commit_effects,
    }
}

fn required_string(attrs: &Map<String, Value>, field: &str) -> Result<String> {
    match attrs.get(field).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => Ok(value.to_owned()),
        _ => {
            let mut errors = ValidationErrors::new();
            for required in ["service", "error_class", "message"] {
                if attrs
                    .get(required)
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    errors.insert(required.to_owned(), vec!["can't be blank".to_owned()]);
                }
            }
            Err(IngestError::Validation(errors))
        }
    }
}

fn validate_context(attrs: &Map<String, Value>) -> Result<()> {
    let Some(context) = attrs.get("context") else {
        return Ok(());
    };
    if !context.is_object() {
        return Ok(());
    }

    let encoded = context.to_string();
    if encoded.len() > MAX_CONTEXT_SIZE {
        Err(IngestError::PayloadTooLarge(format!(
            "context exceeds {MAX_CONTEXT_SIZE} bytes"
        )))
    } else {
        Ok(())
    }
}

fn validate_fingerprint(attrs: &Map<String, Value>) -> Result<Option<Vec<String>>> {
    let Some(fingerprint) = attrs.get("fingerprint") else {
        return Ok(None);
    };
    let Some(items) = fingerprint.as_array() else {
        return Err(fingerprint_error("must be a list of strings"));
    };

    if items.len() > MAX_FINGERPRINT_ELEMENTS {
        return Err(fingerprint_error(&format!(
            "max {MAX_FINGERPRINT_ELEMENTS} elements"
        )));
    }

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(item) = item.as_str() else {
            return Err(fingerprint_error("elements must be strings"));
        };
        if item.chars().count() > MAX_FINGERPRINT_ELEMENT_LEN {
            return Err(fingerprint_error(&format!(
                "elements max {MAX_FINGERPRINT_ELEMENT_LEN} chars"
            )));
        }
        out.push(item.to_owned());
    }

    Ok(Some(out))
}

fn fingerprint_error(message: &str) -> IngestError {
    let mut errors = ValidationErrors::new();
    errors.insert("fingerprint".to_owned(), vec![message.to_owned()]);
    IngestError::Validation(errors)
}

fn optional_string(attrs: &Map<String, Value>, field: &str) -> Option<String> {
    attrs
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn context_json(attrs: &Map<String, Value>) -> Option<String> {
    let context = attrs.get("context")?;
    if context.is_object() {
        Some(context.to_string())
    } else {
        context.as_str().map(ToOwned::to_owned)
    }
}

fn fingerprint_json(fingerprint: Option<&[String]>) -> Option<String> {
    fingerprint.and_then(|fingerprint| serde_json::to_string(fingerprint).ok())
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use canary_core::ids::{ErrorId, EventId};
    use canary_store::Store;
    use serde_json::json;

    use super::*;

    #[test]
    fn ingest_creates_error_group_and_returns_phoenix_summary_shape() -> Result<()> {
        let mut store = migrated_store()?;
        let accepted = ingest(
            &mut store,
            object(&valid_attrs())?,
            &IngestConfig::default(),
            context(
                "ERR-123456789abc",
                "EVT-123456789abc",
                "2026-05-28T20:00:00Z",
            ),
        )?;

        assert_eq!(accepted.id, "ERR-123456789abc");
        assert!(accepted.is_new_class);
        assert_eq!(accepted.group_hash.len(), 64);
        assert_eq!(accepted.post_commit_effects.len(), 3);
        assert_eq!(
            accepted.post_commit_effects[0],
            IngestEffect::BroadcastNewError {
                error_id: "ERR-123456789abc".to_owned(),
                service: "cadence".to_owned(),
            }
        );
        assert_eq!(
            accepted.post_commit_effects[1],
            IngestEffect::CorrelateIncident {
                signal_type: "error_group".to_owned(),
                signal_ref: accepted.group_hash.clone(),
                service: "cadence".to_owned(),
            }
        );
        assert!(matches!(
            &accepted.post_commit_effects[2],
            IngestEffect::EnqueueWebhook { event, payload_json }
                if event == "error.new_class" && payload_json.contains("error.new_class")
        ));

        Ok(())
    }

    #[test]
    fn duplicate_ingest_reuses_group_and_clears_new_class_flag() -> Result<()> {
        let mut store = migrated_store()?;
        let attrs = valid_attrs();
        let attrs = object(&attrs)?;
        let first = ingest(
            &mut store,
            attrs,
            &IngestConfig::default(),
            context(
                "ERR-123456789abc",
                "EVT-123456789abc",
                "2026-05-28T20:00:00Z",
            ),
        )?;
        let second = ingest(
            &mut store,
            attrs,
            &IngestConfig::default(),
            context(
                "ERR-abcdefghijkl",
                "EVT-abcdefghijkl",
                "2026-05-28T20:01:00Z",
            ),
        )?;

        assert_eq!(first.group_hash, second.group_hash);
        assert!(!second.is_new_class);
        assert_eq!(second.post_commit_effects.len(), 2);
        assert!(matches!(
            second.post_commit_effects.as_slice(),
            [
                IngestEffect::BroadcastNewError { .. },
                IngestEffect::CorrelateIncident { .. }
            ]
        ));

        Ok(())
    }

    #[test]
    fn regression_ingest_emits_webhook_effect_after_commit() -> Result<()> {
        let mut store = migrated_store()?;
        let attrs = valid_attrs();
        let attrs = object(&attrs)?;
        let first = ingest(
            &mut store,
            attrs,
            &IngestConfig::default(),
            context(
                "ERR-123456789abc",
                "EVT-123456789abc",
                "2026-05-27T20:00:00Z",
            ),
        )?;
        let second = ingest(
            &mut store,
            attrs,
            &IngestConfig::default(),
            context(
                "ERR-abcdefghijkl",
                "EVT-abcdefghijkl",
                "2026-05-28T20:00:00Z",
            ),
        )?;

        assert_eq!(first.group_hash, second.group_hash);
        assert!(matches!(
            second.post_commit_effects.last(),
            Some(IngestEffect::EnqueueWebhook { event, payload_json })
                if event == "error.regression" && payload_json.contains("error.regression")
        ));

        Ok(())
    }

    #[test]
    fn validation_order_reports_required_fields_before_context_size() -> Result<()> {
        let mut store = migrated_store()?;
        let attrs = json!({
            "service": "svc",
            "context": {"blob": "x".repeat(9_000)},
            "fingerprint": 123
        });

        let err = ingest(
            &mut store,
            object(&attrs)?,
            &IngestConfig::default(),
            context(
                "ERR-123456789abc",
                "EVT-123456789abc",
                "2026-05-28T20:00:00Z",
            ),
        );

        assert!(matches!(err, Err(IngestError::Validation(_))));
        if let Err(IngestError::Validation(errors)) = err {
            assert!(errors.contains_key("error_class"));
            assert!(errors.contains_key("message"));
            assert!(!errors.contains_key("fingerprint"));
        }
        assert_eq!(store.schema_version()?, canary_store_schema_version());

        Ok(())
    }

    #[test]
    fn context_size_is_checked_before_fingerprint_validation() -> Result<()> {
        let mut store = migrated_store()?;
        let attrs = json!({
            "service": "svc",
            "error_class": "RuntimeError",
            "message": "boom",
            "context": {"blob": "x".repeat(9_000)},
            "fingerprint": 123
        });

        let err = ingest(
            &mut store,
            object(&attrs)?,
            &IngestConfig::default(),
            context(
                "ERR-123456789abc",
                "EVT-123456789abc",
                "2026-05-28T20:00:00Z",
            ),
        );

        assert!(matches!(err, Err(IngestError::PayloadTooLarge(_))));
        Ok(())
    }

    #[test]
    fn fingerprint_validation_matches_phoenix_errors() -> Result<()> {
        let mut store = migrated_store()?;
        let attrs = json!({
            "service": "svc",
            "error_class": "RuntimeError",
            "message": "boom",
            "fingerprint": ["ok", 123]
        });

        let err = ingest(
            &mut store,
            object(&attrs)?,
            &IngestConfig::default(),
            context(
                "ERR-123456789abc",
                "EVT-123456789abc",
                "2026-05-28T20:00:00Z",
            ),
        );

        assert!(matches!(err, Err(IngestError::Validation(_))));
        if let Err(IngestError::Validation(errors)) = err {
            assert_eq!(errors["fingerprint"], ["elements must be strings"]);
        }
        Ok(())
    }

    #[test]
    fn truncates_message_stack_and_persists_defaults_and_classification() -> Result<()> {
        let mut store = migrated_store()?;
        let attrs = json!({
            "service": "svc",
            "error_class": "DBConnection.ConnectionError",
            "message": "m".repeat(5_000),
            "stack_trace": "s".repeat(40_000)
        });

        ingest(
            &mut store,
            object(&attrs)?,
            &IngestConfig::default(),
            context(
                "ERR-123456789abc",
                "EVT-123456789abc",
                "2026-05-28T20:00:00Z",
            ),
        )?;

        Ok(())
    }

    fn migrated_store() -> Result<Store> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        Ok(store)
    }

    fn valid_attrs() -> Value {
        json!({
            "service": "cadence",
            "error_class": "RuntimeError",
            "message": "something went wrong"
        })
    }

    fn object(value: &Value) -> Result<&Map<String, Value>> {
        value.as_object().ok_or_else(|| {
            IngestError::PayloadTooLarge("test payload was not an object".to_owned())
        })
    }

    fn context(error_id: &str, event_id: &str, now: &str) -> IngestContext {
        IngestContext {
            error_id: ErrorId::from_str(error_id).unwrap_or_else(|_| ErrorId::generate()),
            event_id: EventId::from_str(event_id).unwrap_or_else(|_| EventId::generate()),
            now: now.to_owned(),
        }
    }

    const fn canary_store_schema_version() -> u32 {
        2026042200
    }
}
