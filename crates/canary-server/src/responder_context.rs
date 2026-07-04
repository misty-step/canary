//! Server-side responder context adapters.
//!
//! Store read models intentionally preserve raw durable rows. This module owns
//! the HTTP boundary that narrows those rows into redacted envelopes before an
//! external responder receives them.

use canary_core::{
    query::IncidentDetail,
    redaction::{REDACTED, scrub_string, sensitive_key},
};
use canary_store::VerifiedApiKey;
use serde_json::{Value, json};

pub(crate) const INCIDENT_CONTEXT_SCHEMA: &str = "canary.responder_context.incident.v1";
const MAX_CONTEXT_STRING_CHARS: usize = 1_024;
const MAX_METADATA_BYTES: usize = 4_096;
const MAX_EVIDENCE_LINK_CHARS: usize = 512;
const MAX_INCIDENT_SIGNALS: usize = 25;
const MAX_INCIDENT_ANNOTATIONS: usize = 20;
const MAX_INCIDENT_CLAIMS: usize = 20;
const MAX_INCIDENT_TIMELINE_EVENTS: usize = 5;

pub(crate) fn incident_context_response(
    detail: IncidentDetail,
    authority: &VerifiedApiKey,
    audit_event_id: Option<String>,
) -> Value {
    let service = detail.incident.service.clone();
    let incident_id = detail.incident.id.clone();
    let signals_returned = detail.signals.len();
    let annotations_returned = detail.annotations.len();
    let claims_returned = detail.claims.len();
    let timeline_returned = detail.recent_timeline_events.len();
    let signals_truncated = detail.signals_truncated;
    let annotations_truncated = detail.annotations_truncated;

    let mut body = serde_json::to_value(detail).unwrap_or_else(|_| {
        json!({
            "summary": "Incident context serialization failed.",
            "incident": {"id": incident_id, "service": service}
        })
    });
    redact_context_value(&mut body);
    if let Some(object) = body.as_object_mut() {
        object.insert(
            "context_envelope".to_owned(),
            json!({
                "schema": INCIDENT_CONTEXT_SCHEMA,
                "tenant_id": authority.tenant_id,
                "project_id": authority.project_id,
                "service": service,
                "subject": {
                    "type": "incident",
                    "id": incident_id
                },
                "retention": {
                    "class": "audit",
                    "audit_event": "responder.context_read"
                },
                "privacy_policy": {
                    "classification": "redacted",
                    "redaction_rules": [
                        "bearer_token",
                        "canary_api_key",
                        "email",
                        "sensitive_key_value"
                    ],
                    "max_string_chars": MAX_CONTEXT_STRING_CHARS,
                    "max_metadata_bytes": MAX_METADATA_BYTES,
                    "max_evidence_link_chars": MAX_EVIDENCE_LINK_CHARS
                },
                "bounds": {
                    "signals": {
                        "returned": signals_returned,
                        "max": MAX_INCIDENT_SIGNALS,
                        "truncated": signals_truncated
                    },
                    "annotations": {
                        "returned": annotations_returned,
                        "max": MAX_INCIDENT_ANNOTATIONS,
                        "truncated": annotations_truncated
                    },
                    "claims": {
                        "returned": claims_returned,
                        "max": MAX_INCIDENT_CLAIMS
                    },
                    "recent_timeline_events": {
                        "returned": timeline_returned,
                        "max": MAX_INCIDENT_TIMELINE_EVENTS
                    }
                },
                "audit_event_id": audit_event_id
            }),
        );
    }
    body
}

fn redact_context_value(value: &mut Value) {
    match value {
        Value::String(value) => {
            *value = bounded_string(scrub_string(value), MAX_CONTEXT_STRING_CHARS);
        }
        Value::Array(values) => {
            for value in values {
                redact_context_value(value);
            }
        }
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if sensitive_key(key) {
                    *value = Value::String(REDACTED.to_owned());
                } else {
                    redact_context_value(value);
                    if key == "metadata" {
                        bound_json_value(value, MAX_METADATA_BYTES);
                    } else if key == "evidence_links" {
                        bound_evidence_links(value);
                    }
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn bound_json_value(value: &mut Value, max_bytes: usize) {
    let Ok(serialized) = serde_json::to_vec(value) else {
        *value = truncated_json_value(0, max_bytes);
        return;
    };
    if serialized.len() > max_bytes {
        *value = truncated_json_value(serialized.len(), max_bytes);
    }
}

fn bound_evidence_links(value: &mut Value) {
    let Value::Array(values) = value else {
        return;
    };
    for value in values {
        let Value::String(link) = value else {
            continue;
        };
        *link = bounded_string(link.clone(), MAX_EVIDENCE_LINK_CHARS);
    }
}

fn truncated_json_value(original_size_bytes: usize, limit_bytes: usize) -> Value {
    json!({
        "redacted": "[TRUNCATED]",
        "original_size_bytes": original_size_bytes,
        "limit_bytes": limit_bytes
    })
}

fn bounded_string(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str(" [TRUNCATED]");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_core::query::{
        IncidentActionBrief, IncidentActionRecommendation, IncidentActionSignalCounts,
        IncidentAnnotation, IncidentDetailIncident, RemediationClaim,
    };
    use serde_json::json;

    fn authority() -> VerifiedApiKey {
        VerifiedApiKey {
            id: "KEY-test".to_owned(),
            name: "test".to_owned(),
            scope: "responder-write".to_owned(),
            tenant_id: "TENANT-test".to_owned(),
            project_id: "PROJECT-test".to_owned(),
            service: Some("svc".to_owned()),
        }
    }

    #[test]
    fn incident_context_response_redacts_metadata_and_claim_evidence() {
        let detail = IncidentDetail {
            summary: "svc incident for alice@example.com".to_owned(),
            incident: IncidentDetailIncident {
                id: "INC-test".to_owned(),
                service: "svc".to_owned(),
                state: "investigating".to_owned(),
                severity: "medium".to_owned(),
                title: Some("Bearer abc.def".to_owned()),
                opened_at: "2026-05-28T20:00:00Z".to_owned(),
                resolved_at: None,
                signal_count: 0,
            },
            signals: Vec::new(),
            signals_truncated: false,
            annotations: vec![IncidentAnnotation {
                id: "ANN-test".to_owned(),
                subject_type: Some("incident".to_owned()),
                subject_id: Some("INC-test".to_owned()),
                incident_id: Some("INC-test".to_owned()),
                group_hash: None,
                agent: "bob@example.com".to_owned(),
                action: "triaged".to_owned(),
                metadata: Some(json!({"api_key": "sk_live_secret"})),
                created_at: "2026-05-28T20:01:00Z".to_owned(),
            }],
            claims: vec![RemediationClaim {
                id: "CLM-test".to_owned(),
                tenant_id: "TENANT-test".to_owned(),
                project_id: "PROJECT-test".to_owned(),
                service: Some("svc".to_owned()),
                subject_type: "incident".to_owned(),
                subject_id: "INC-test".to_owned(),
                owner: "alice@example.com".to_owned(),
                purpose: "token=sk_live_secret".to_owned(),
                state: "claimed".to_owned(),
                idempotency_key: "idem".to_owned(),
                evidence_links: vec!["https://example.test?token=sk_live_secret".to_owned()],
                created_at: "2026-05-28T20:01:00Z".to_owned(),
                updated_at: "2026-05-28T20:01:00Z".to_owned(),
                expires_at: "2026-05-28T20:10:00Z".to_owned(),
                released_at: None,
                completed_at: None,
            }],
            annotations_truncated: false,
            recent_timeline_events: Vec::new(),
            action_brief: IncidentActionBrief {
                summary: "brief".to_owned(),
                recommendation: IncidentActionRecommendation {
                    action: "watch".to_owned(),
                    reason: "no-op".to_owned(),
                },
                signal_counts: IncidentActionSignalCounts {
                    active: 0,
                    resolved: 0,
                    visible: 0,
                    total: 0,
                },
                signals_truncated: false,
                latest_annotation: None,
                current_claim: None,
            },
        };

        let value = incident_context_response(detail, &authority(), Some("EVT-test".to_owned()));
        let rendered = serde_json::to_string(&value).unwrap_or_default();

        assert!(!rendered.contains("sk_live_secret"));
        assert!(!rendered.contains("alice@example.com"));
        assert!(!rendered.contains("bob@example.com"));
        assert_eq!(value["annotations"][0]["metadata"]["api_key"], REDACTED);
        assert_eq!(
            value["claims"][0]["evidence_links"][0],
            "https://example.test?token=[REDACTED]"
        );
        assert_eq!(value["context_envelope"]["schema"], INCIDENT_CONTEXT_SCHEMA);
    }
}
