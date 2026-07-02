//! Axum adapter for incident escalation routes.
//!
//! Escalation is an orthogonal overlay on top of the deterministic incident
//! state machine (see `canary_store::incidents`): it never appears as a
//! value of `incidents.state`. This module owns only the HTTP-specific
//! translation (auth, body parsing, problem responses, webhook enqueue); the
//! escalation invariants themselves live in `canary_store::escalation`.

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
};
use canary_core::query::IncidentEscalation;
use canary_http::{
    problem_details::{
        ProblemCode, ProblemDetails, internal_problem, not_found_problem,
        payload_too_large_problem, validation_problem,
    },
    request::decode_json_object,
};
use canary_ingest::{IngestEffect, ValidationErrors};
use canary_store::{DeescalationRequest, EscalationError, EscalationInsert};
use serde_json::{Map, Value, json};

use crate::{
    IngestState,
    body_fields::{optional_string, required_string},
    http_contract::{check_content_length, json_status_response, problem_response},
    require_responder_write_scope,
    server_time::current_rfc3339,
};

struct EscalateRequest {
    reason: String,
    owner: String,
    purpose: String,
    idempotency_key: String,
}

struct DeescalateRequestBody {
    owner: String,
    reason: Option<String>,
}

pub(crate) async fn escalate_incident(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(incident_id): Path<String>,
    body: Bytes,
) -> Response<Body> {
    if let Err(problem) = check_content_length(&headers) {
        return problem_response(payload_too_large_problem(problem.detail));
    }
    let authority = match require_responder_write_scope(&state, &headers) {
        Ok(authority) => authority,
        Err(problem) => return problem_response(*problem),
    };
    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let request = match parse_escalate_request(attrs) {
        Ok(request) => request,
        Err(problem) => return problem_response(*problem),
    };
    let now = current_rfc3339();

    let outcome = {
        let mut store = match state.lock_store() {
            Ok(store) => store,
            Err(_) => return problem_response(internal_problem()),
        };
        match store.escalate_incident(EscalationInsert {
            incident_id: incident_id.clone(),
            tenant_id: authority.tenant_id.clone(),
            project_id: authority.project_id.clone(),
            service: authority.service.clone(),
            owner: request.owner,
            reason: request.reason,
            purpose: request.purpose,
            idempotency_key: request.idempotency_key,
            event_id: canary_core::ids::EventId::generate().into_string(),
            now: now.clone(),
        }) {
            Ok(outcome) => outcome,
            Err(EscalationError::NotFound) => {
                return problem_response(not_found_problem(format!(
                    "Incident {incident_id} not found."
                )));
            }
            Err(EscalationError::AlreadyResolved) => {
                return problem_response(escalation_already_resolved_problem());
            }
            Err(EscalationError::InvalidEscalation) => {
                return problem_response(invalid_escalation_problem());
            }
            Err(EscalationError::Sqlite(_)) => return problem_response(internal_problem()),
        }
    };

    if outcome.created {
        enqueue_escalation_webhook(
            &state,
            "incident.escalated",
            &outcome.escalation,
            &authority.tenant_id,
            &authority.project_id,
            &outcome.service,
            &now,
        );
        json_status_response(
            StatusCode::CREATED.as_u16(),
            escalation_response(&outcome.escalation),
        )
    } else {
        json_status_response(
            StatusCode::OK.as_u16(),
            escalation_response(&outcome.escalation),
        )
    }
}

pub(crate) async fn deescalate_incident(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(incident_id): Path<String>,
    body: Bytes,
) -> Response<Body> {
    if let Err(problem) = check_content_length(&headers) {
        return problem_response(payload_too_large_problem(problem.detail));
    }
    let authority = match require_responder_write_scope(&state, &headers) {
        Ok(authority) => authority,
        Err(problem) => return problem_response(*problem),
    };
    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let request = match parse_deescalate_request(attrs) {
        Ok(request) => request,
        Err(problem) => return problem_response(*problem),
    };
    let now = current_rfc3339();

    let outcome = {
        let mut store = match state.lock_store() {
            Ok(store) => store,
            Err(_) => return problem_response(internal_problem()),
        };
        match store.deescalate_incident(DeescalationRequest {
            incident_id: incident_id.clone(),
            tenant_id: authority.tenant_id.clone(),
            project_id: authority.project_id.clone(),
            service: authority.service.clone(),
            owner: request.owner,
            reason: request.reason,
            event_id: canary_core::ids::EventId::generate().into_string(),
            now: now.clone(),
        }) {
            Ok(outcome) => outcome,
            Err(EscalationError::NotFound) => {
                return problem_response(not_found_problem(format!(
                    "Incident {incident_id} not found."
                )));
            }
            Err(EscalationError::AlreadyResolved) => {
                return problem_response(escalation_already_resolved_problem());
            }
            Err(EscalationError::InvalidEscalation) => {
                return problem_response(invalid_escalation_problem());
            }
            Err(EscalationError::Sqlite(_)) => return problem_response(internal_problem()),
        }
    };

    if outcome.changed {
        enqueue_escalation_webhook(
            &state,
            "incident.deescalated",
            &outcome.escalation,
            &authority.tenant_id,
            &authority.project_id,
            &outcome.service,
            &now,
        );
    }

    json_status_response(
        StatusCode::OK.as_u16(),
        escalation_response(&outcome.escalation),
    )
}

fn escalation_response(escalation: &IncidentEscalation) -> Value {
    json!({ "escalation": escalation })
}

fn enqueue_escalation_webhook(
    state: &IngestState,
    event: &str,
    escalation: &IncidentEscalation,
    tenant_id: &str,
    project_id: &str,
    service: &str,
    timestamp: &str,
) {
    let payload = json!({
        "event": event,
        "tenant_id": tenant_id,
        "project_id": project_id,
        "service": service,
        "escalation": escalation,
        "timestamp": timestamp,
    });
    let _ = state.handle_effects(&[IngestEffect::EnqueueWebhook {
        event: event.to_owned(),
        payload_json: payload.to_string(),
    }]);
}

fn parse_escalate_request(
    attrs: Map<String, Value>,
) -> Result<EscalateRequest, Box<ProblemDetails>> {
    let mut errors = ValidationErrors::new();
    let reason = required_string(&attrs, "reason", &mut errors);
    let owner = required_string(&attrs, "owner", &mut errors);
    let purpose = required_string(&attrs, "purpose", &mut errors);
    let idempotency_key = required_string(&attrs, "idempotency_key", &mut errors);
    if !errors.is_empty() {
        return Err(Box::new(validation_problem("Invalid escalation.", &errors)));
    }
    Ok(EscalateRequest {
        reason: reason.unwrap_or_default(),
        owner: owner.unwrap_or_default(),
        purpose: purpose.unwrap_or_default(),
        idempotency_key: idempotency_key.unwrap_or_default(),
    })
}

fn parse_deescalate_request(
    attrs: Map<String, Value>,
) -> Result<DeescalateRequestBody, Box<ProblemDetails>> {
    let mut errors = ValidationErrors::new();
    let owner = required_string(&attrs, "owner", &mut errors);
    if !errors.is_empty() {
        return Err(Box::new(validation_problem(
            "Invalid deescalation.",
            &errors,
        )));
    }
    Ok(DeescalateRequestBody {
        owner: owner.unwrap_or_default(),
        reason: optional_string(attrs.get("reason")),
    })
}

fn escalation_already_resolved_problem() -> ProblemDetails {
    ProblemDetails::new(
        409,
        ProblemCode::Other("incident_already_resolved".to_owned()),
        "Cannot escalate a resolved incident.",
        None,
    )
}

fn invalid_escalation_problem() -> ProblemDetails {
    validation_problem("Invalid escalation.", json!({}))
}
