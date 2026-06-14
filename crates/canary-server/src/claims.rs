//! Axum adapter for durable remediation-claim coordination routes.

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::{
    problem_details::{
        ProblemDetails, internal_problem, not_found_problem, payload_too_large_problem,
        validation_problem,
    },
    request::decode_json_object,
};
use canary_ingest::{IngestEffect, ValidationErrors};
use canary_store::{ClaimError, ClaimInsert, ClaimListOptions, ClaimTransition, VerifiedApiKey};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use time::Duration;

use crate::{
    IngestState,
    body_fields::{optional_positive_i64, required_string},
    http_contract::{check_content_length, json_status_response, problem_response},
    require_query_limited_admin_scope, require_read_scope,
    server_time::{current_rfc3339, current_utc, format_rfc3339},
};

#[derive(Deserialize)]
pub(crate) struct ClaimListParams {
    subject_type: Option<String>,
    subject_id: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
}

struct ClaimCreate {
    subject_type: String,
    subject_id: String,
    owner: String,
    purpose: String,
    idempotency_key: String,
    ttl_ms: i64,
    evidence_links: Vec<String>,
}

struct ClaimTransitionRequest {
    owner: String,
    state: String,
    evidence_links: Vec<String>,
}

struct ClaimReleaseRequest {
    owner: String,
}

pub(crate) async fn list_claims(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<ClaimListParams>,
) -> Response<Body> {
    let authority = match require_read_scope(&state, &headers) {
        Ok(authority) => authority,
        Err(problem) => return problem_response(*problem),
    };
    let Some(subject_type) = params.subject_type.filter(|value| !value.is_empty()) else {
        return problem_response(claim_validation_problem("subject_type", "is required"));
    };
    let Some(subject_id) = params.subject_id.filter(|value| !value.is_empty()) else {
        return problem_response(claim_validation_problem("subject_id", "is required"));
    };

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.claims(ClaimListOptions {
        tenant_id: Some(authority.tenant_id),
        project_id: Some(authority.project_id),
        service: authority.service,
        subject_type,
        subject_id,
        limit: params.limit,
        cursor: params.cursor,
    }) {
        Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
        Err(ClaimError::InvalidSubjectType) => problem_response(invalid_claim_subject_problem()),
        Err(ClaimError::InvalidLimit) => problem_response(claim_validation_problem(
            "limit",
            "must be between 1 and 50",
        )),
        Err(ClaimError::InvalidCursor) => {
            problem_response(claim_validation_problem("cursor", "is invalid"))
        }
        Err(ClaimError::NotFound) => {
            problem_response(not_found_problem("Claim subject not found."))
        }
        Err(ClaimError::Sqlite(_)) => problem_response(internal_problem()),
        Err(
            ClaimError::InvalidState
            | ClaimError::InvalidClaim
            | ClaimError::Conflict(_)
            | ClaimError::InvalidTransition,
        ) => problem_response(internal_problem()),
    }
}

pub(crate) async fn show_claim(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(claim_id): Path<String>,
) -> Response<Body> {
    let authority = match require_read_scope(&state, &headers) {
        Ok(authority) => authority,
        Err(problem) => return problem_response(*problem),
    };
    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.claim_scoped(
        &claim_id,
        &authority.tenant_id,
        &authority.project_id,
        authority.service.as_deref(),
    ) {
        Ok(Some(claim)) => json_status_response(StatusCode::OK.as_u16(), claim),
        Ok(None) | Err(ClaimError::NotFound) => {
            problem_response(not_found_problem(format!("Claim {claim_id} not found.")))
        }
        Err(ClaimError::Sqlite(_)) => problem_response(internal_problem()),
        Err(
            ClaimError::InvalidSubjectType
            | ClaimError::InvalidState
            | ClaimError::InvalidClaim
            | ClaimError::InvalidLimit
            | ClaimError::InvalidCursor
            | ClaimError::Conflict(_)
            | ClaimError::InvalidTransition,
        ) => problem_response(internal_problem()),
    }
}

pub(crate) async fn create_claim(
    State(state): State<IngestState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if let Err(problem) = check_content_length(&headers) {
        return problem_response(payload_too_large_problem(problem.detail));
    }
    let authority = match require_query_limited_admin_scope(&state, &headers) {
        Ok(authority) => authority,
        Err(problem) => return problem_response(*problem),
    };
    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let request = match parse_claim_create(attrs) {
        Ok(request) => request,
        Err(problem) => return problem_response(*problem),
    };
    let now_utc = current_utc();
    let now = format_rfc3339(now_utc);
    let Some(expires_at) = now_utc
        .checked_add(Duration::milliseconds(request.ttl_ms))
        .map(format_rfc3339)
    else {
        return problem_response(claim_validation_problem("ttl_ms", "is too large"));
    };
    let outcome = {
        let mut store = match state.lock_store() {
            Ok(store) => store,
            Err(_) => return problem_response(internal_problem()),
        };
        match store.create_claim_outcome(ClaimInsert {
            id: canary_core::ids::ClaimId::generate().into_string(),
            event_id: canary_core::ids::EventId::generate().into_string(),
            tenant_id: authority.tenant_id.clone(),
            project_id: authority.project_id.clone(),
            service: authority.service.clone(),
            subject_type: request.subject_type,
            subject_id: request.subject_id,
            owner: request.owner,
            purpose: request.purpose,
            idempotency_key: request.idempotency_key,
            evidence_links: request.evidence_links,
            now: now.clone(),
            expires_at,
        }) {
            Ok(outcome) => outcome,
            Err(ClaimError::Conflict(current)) => {
                return problem_response(claim_conflict_problem(*current));
            }
            Err(ClaimError::InvalidSubjectType) => {
                return problem_response(invalid_claim_subject_problem());
            }
            Err(ClaimError::InvalidClaim | ClaimError::InvalidState) => {
                return problem_response(invalid_claim_problem());
            }
            Err(ClaimError::InvalidLimit | ClaimError::InvalidCursor) => {
                return problem_response(internal_problem());
            }
            Err(ClaimError::NotFound) => {
                return problem_response(not_found_problem("Claim subject not found."));
            }
            Err(ClaimError::Sqlite(_)) => return problem_response(internal_problem()),
            Err(ClaimError::InvalidTransition) => return problem_response(internal_problem()),
        }
    };
    if outcome.created {
        enqueue_claim_webhook(&state, "remediation_claim.created", &outcome.claim, &now);
        json_status_response(StatusCode::CREATED.as_u16(), outcome.claim)
    } else {
        json_status_response(StatusCode::OK.as_u16(), outcome.claim)
    }
}

pub(crate) async fn transition_claim(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(claim_id): Path<String>,
    body: Bytes,
) -> Response<Body> {
    if let Err(problem) = check_content_length(&headers) {
        return problem_response(payload_too_large_problem(problem.detail));
    }
    let authority = match require_query_limited_admin_scope(&state, &headers) {
        Ok(authority) => authority,
        Err(problem) => return problem_response(*problem),
    };
    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let request = match parse_claim_transition(attrs) {
        Ok(request) => request,
        Err(problem) => return problem_response(*problem),
    };
    transition_claim_to(state, authority, claim_id, request).await
}

pub(crate) async fn release_claim(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(claim_id): Path<String>,
    body: Bytes,
) -> Response<Body> {
    if let Err(problem) = check_content_length(&headers) {
        return problem_response(payload_too_large_problem(problem.detail));
    }
    let authority = match require_query_limited_admin_scope(&state, &headers) {
        Ok(authority) => authority,
        Err(problem) => return problem_response(*problem),
    };
    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let request = match parse_claim_release(attrs) {
        Ok(request) => request,
        Err(problem) => return problem_response(*problem),
    };
    transition_claim_to(
        state,
        authority,
        claim_id,
        ClaimTransitionRequest {
            owner: request.owner,
            state: "released".to_owned(),
            evidence_links: Vec::new(),
        },
    )
    .await
}

async fn transition_claim_to(
    state: IngestState,
    authority: VerifiedApiKey,
    claim_id: String,
    request: ClaimTransitionRequest,
) -> Response<Body> {
    let now = current_rfc3339();
    let claim = {
        let mut store = match state.lock_store() {
            Ok(store) => store,
            Err(_) => return problem_response(internal_problem()),
        };
        match store.transition_claim(ClaimTransition {
            event_id: canary_core::ids::EventId::generate().into_string(),
            claim_id: claim_id.clone(),
            tenant_id: authority.tenant_id,
            project_id: authority.project_id,
            service: authority.service,
            owner: request.owner,
            state: request.state,
            evidence_links: request.evidence_links,
            now: now.clone(),
        }) {
            Ok(claim) => claim,
            Err(ClaimError::NotFound) => {
                return problem_response(not_found_problem(format!("Claim {claim_id} not found.")));
            }
            Err(ClaimError::InvalidState) => return problem_response(invalid_claim_state_problem()),
            Err(ClaimError::InvalidTransition) => {
                return problem_response(invalid_claim_transition_problem());
            }
            Err(ClaimError::Sqlite(_)) => return problem_response(internal_problem()),
            Err(
                ClaimError::InvalidSubjectType
                | ClaimError::InvalidClaim
                | ClaimError::InvalidLimit
                | ClaimError::InvalidCursor
                | ClaimError::Conflict(_),
            ) => return problem_response(internal_problem()),
        }
    };
    let event = if claim.state == "released" {
        "remediation_claim.released"
    } else {
        "remediation_claim.updated"
    };
    enqueue_claim_webhook(&state, event, &claim, &now);
    json_status_response(StatusCode::OK.as_u16(), claim)
}

fn parse_claim_create(attrs: Map<String, Value>) -> Result<ClaimCreate, Box<ProblemDetails>> {
    let mut errors = ValidationErrors::new();
    let subject_type = required_string(&attrs, "subject_type", &mut errors);
    let subject_id = required_string(&attrs, "subject_id", &mut errors);
    let owner = required_string(&attrs, "owner", &mut errors);
    let purpose = required_string(&attrs, "purpose", &mut errors);
    let idempotency_key = required_string(&attrs, "idempotency_key", &mut errors);
    let ttl_ms = optional_positive_i64(&attrs, "ttl_ms", 0, &mut errors);
    if !attrs.contains_key("ttl_ms") {
        errors.insert("ttl_ms".to_owned(), vec!["is required".to_owned()]);
    }
    let evidence_links = optional_string_array(&attrs, "evidence_links", &mut errors);
    if let Some(subject_type) = subject_type.as_deref()
        && !canary_store::claim_subject_types().contains(&subject_type)
    {
        errors.insert(
            "subject_type".to_owned(),
            vec!["must be one of incident, error_group, target, monitor".to_owned()],
        );
    }
    if !errors.is_empty() {
        return Err(Box::new(validation_problem("Invalid claim.", &errors)));
    }
    Ok(ClaimCreate {
        subject_type: subject_type.unwrap_or_default(),
        subject_id: subject_id.unwrap_or_default(),
        owner: owner.unwrap_or_default(),
        purpose: purpose.unwrap_or_default(),
        idempotency_key: idempotency_key.unwrap_or_default(),
        ttl_ms,
        evidence_links,
    })
}

fn parse_claim_transition(
    attrs: Map<String, Value>,
) -> Result<ClaimTransitionRequest, Box<ProblemDetails>> {
    let mut errors = ValidationErrors::new();
    let owner = required_string(&attrs, "owner", &mut errors);
    let state = required_string(&attrs, "state", &mut errors);
    let evidence_links = optional_string_array(&attrs, "evidence_links", &mut errors);
    if let Some(state) = state.as_deref()
        && !canary_core::query::claim_state_is_valid(state)
    {
        errors.insert(
            "state".to_owned(),
            vec!["must be a remediation claim state".to_owned()],
        );
    }
    if !errors.is_empty() {
        return Err(Box::new(validation_problem(
            "Invalid claim transition.",
            &errors,
        )));
    }
    Ok(ClaimTransitionRequest {
        owner: owner.unwrap_or_default(),
        state: state.unwrap_or_default(),
        evidence_links,
    })
}

fn parse_claim_release(
    attrs: Map<String, Value>,
) -> Result<ClaimReleaseRequest, Box<ProblemDetails>> {
    let mut errors = ValidationErrors::new();
    let owner = required_string(&attrs, "owner", &mut errors);
    if !errors.is_empty() {
        return Err(Box::new(validation_problem(
            "Invalid claim release.",
            &errors,
        )));
    }
    Ok(ClaimReleaseRequest {
        owner: owner.unwrap_or_default(),
    })
}

fn optional_string_array(
    attrs: &Map<String, Value>,
    key: &str,
    errors: &mut ValidationErrors,
) -> Vec<String> {
    match attrs.get(key) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) => {
            let mut strings = Vec::new();
            for (index, value) in values.iter().enumerate() {
                match value {
                    Value::String(value) if !value.is_empty() => strings.push(value.clone()),
                    _ => {
                        errors.insert(
                            format!("{key}.{index}"),
                            vec!["must be a non-empty string".to_owned()],
                        );
                    }
                }
            }
            strings
        }
        Some(_) => {
            errors.insert(key.to_owned(), vec!["must be an array".to_owned()]);
            Vec::new()
        }
    }
}

fn enqueue_claim_webhook(
    state: &IngestState,
    event: &str,
    claim: &canary_core::query::RemediationClaim,
    timestamp: &str,
) {
    let payload = json!({
        "event": event,
        "tenant_id": claim.tenant_id,
        "project_id": claim.project_id,
        "service": claim.service,
        "claim": claim,
        "timestamp": timestamp,
    });
    let _ = state.handle_effects(&[IngestEffect::EnqueueWebhook {
        event: event.to_owned(),
        payload_json: payload.to_string(),
    }]);
}

fn claim_validation_problem(field: &str, message: &str) -> ProblemDetails {
    validation_problem("Invalid claim.", json!({field: [message]}))
}

fn invalid_claim_problem() -> ProblemDetails {
    validation_problem("Invalid claim.", json!({}))
}

fn invalid_claim_subject_problem() -> ProblemDetails {
    validation_problem(
        "Unknown subject_type.",
        json!({"subject_type": ["must be one of incident, error_group, target, monitor"]}),
    )
}

fn invalid_claim_state_problem() -> ProblemDetails {
    validation_problem(
        "Invalid claim state.",
        json!({"state": ["must be one of claimed, investigating, fix_proposed, verified, dismissed, expired, released"]}),
    )
}

fn invalid_claim_transition_problem() -> ProblemDetails {
    validation_problem(
        "Invalid claim transition.",
        json!({"state": ["terminal claims cannot be reopened"]}),
    )
}

fn claim_conflict_problem(current: canary_core::query::RemediationClaimSummary) -> ProblemDetails {
    ProblemDetails::new(
        409,
        canary_http::problem_details::ProblemCode::Other("claim_conflict".to_owned()),
        "Subject already has an active remediation claim. Release or complete the current claim before creating another active claim.",
        None,
    )
    .with_extra("current_claim", json!(current))
}
