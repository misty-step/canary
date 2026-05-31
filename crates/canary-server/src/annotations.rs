//! Axum adapter for Canary's annotation coordination routes.
//!
//! Annotation routing has its own permission split and webhook side effect. The
//! shared server primitives still own auth, rate limiting, and response
//! serialization; this module owns only the annotation-specific translation.

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::{
    problem_details::{
        ProblemDetails, annotation_invalid_cursor_problem, annotation_missing_subject_problem,
        internal_problem, invalid_annotation_limit_problem, invalid_annotation_problem,
        invalid_annotation_subject_type_problem, not_found_problem, payload_too_large_problem,
        validation_problem,
    },
    request::{MAX_JSON_BODY_BYTES, decode_json_object},
};
use canary_ingest::{IngestEffect, ValidationErrors};
use canary_store::{AnnotationError, AnnotationInsert, AnnotationPageOptions};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    IngestState, current_rfc3339, json_status_response, problem_response,
    require_query_limited_admin_scope, require_read_scope, require_scope,
};
use canary_http::auth::Permission;

#[derive(Deserialize)]
pub(crate) struct AnnotationPageParams {
    subject_type: Option<String>,
    subject_id: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
}

struct AnnotationCreate {
    subject_type: String,
    subject_id: String,
    agent: String,
    action: String,
    metadata: Option<Value>,
}

pub(crate) async fn list_incident_annotations(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(incident_id): Path<String>,
) -> Response<Body> {
    list_annotations_for_subject(
        state,
        headers,
        "incident",
        incident_id,
        "Incident not found.",
    )
}

pub(crate) async fn create_incident_annotation(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(incident_id): Path<String>,
    body: Bytes,
) -> Response<Body> {
    create_annotation_for_subject(
        state,
        headers,
        body,
        "incident",
        incident_id,
        "Incident not found.",
    )
}

pub(crate) async fn list_group_annotations(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(group_hash): Path<String>,
) -> Response<Body> {
    list_annotations_for_subject(
        state,
        headers,
        "error_group",
        group_hash,
        "Error group not found.",
    )
}

pub(crate) async fn create_group_annotation(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(group_hash): Path<String>,
    body: Bytes,
) -> Response<Body> {
    create_annotation_for_subject(
        state,
        headers,
        body,
        "error_group",
        group_hash,
        "Error group not found.",
    )
}

pub(crate) async fn list_annotations(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<AnnotationPageParams>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }
    let Some(subject_type) = params.subject_type.filter(|value| !value.is_empty()) else {
        return problem_response(annotation_missing_subject_problem("subject_type"));
    };
    let Some(subject_id) = params.subject_id.filter(|value| !value.is_empty()) else {
        return problem_response(annotation_missing_subject_problem("subject_id"));
    };

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.annotation_page(AnnotationPageOptions {
        subject_type,
        subject_id,
        limit: params.limit,
        cursor: params.cursor,
    }) {
        Ok(response) => json_status_response(StatusCode::OK.as_u16(), response),
        Err(AnnotationError::NotFound) => problem_response(not_found_problem("Subject not found.")),
        Err(AnnotationError::InvalidSubjectType) => {
            problem_response(invalid_annotation_subject_type_problem())
        }
        Err(AnnotationError::InvalidLimit) => problem_response(invalid_annotation_limit_problem()),
        Err(AnnotationError::InvalidCursor) => {
            problem_response(annotation_invalid_cursor_problem())
        }
        Err(AnnotationError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

pub(crate) async fn create_annotation(
    State(state): State<IngestState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }
    if let Err(problem) = crate::check_content_length(&headers) {
        return problem_response(*problem);
    }
    if body.len() as u64 > MAX_JSON_BODY_BYTES {
        return problem_response(payload_too_large_problem(
            "Request body exceeds 100KB limit.",
        ));
    }
    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let request = match parse_annotation_create(attrs, None) {
        Ok(request) => request,
        Err(problem) => return problem_response(*problem),
    };

    create_annotation_request(state, request, "Subject not found.")
}

fn list_annotations_for_subject(
    state: IngestState,
    headers: HeaderMap,
    subject_type: &'static str,
    subject_id: String,
    not_found_detail: &'static str,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }
    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.annotations(subject_type, &subject_id) {
        Ok(response) => json_status_response(StatusCode::OK.as_u16(), response),
        Err(AnnotationError::NotFound) => problem_response(not_found_problem(not_found_detail)),
        Err(AnnotationError::InvalidSubjectType) => {
            problem_response(invalid_annotation_subject_type_problem())
        }
        Err(AnnotationError::InvalidLimit | AnnotationError::InvalidCursor) => {
            problem_response(internal_problem())
        }
        Err(AnnotationError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

fn create_annotation_for_subject(
    state: IngestState,
    headers: HeaderMap,
    body: Bytes,
    subject_type: &'static str,
    subject_id: String,
    not_found_detail: &'static str,
) -> Response<Body> {
    if let Err(problem) = require_query_limited_admin_scope(&state, &headers) {
        return problem_response(*problem);
    }
    if let Err(problem) = crate::check_content_length(&headers) {
        return problem_response(*problem);
    }
    if body.len() as u64 > MAX_JSON_BODY_BYTES {
        return problem_response(payload_too_large_problem(
            "Request body exceeds 100KB limit.",
        ));
    }
    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let request = match parse_annotation_create(attrs, Some((subject_type, subject_id))) {
        Ok(request) => request,
        Err(problem) => return problem_response(*problem),
    };

    create_annotation_request(state, request, not_found_detail)
}

fn create_annotation_request(
    state: IngestState,
    request: AnnotationCreate,
    not_found_detail: &'static str,
) -> Response<Body> {
    let annotation = {
        let mut store = match state.store.lock() {
            Ok(store) => store,
            Err(_) => return problem_response(internal_problem()),
        };
        match store.create_annotation(AnnotationInsert {
            id: canary_core::ids::AnnotationId::generate().into_string(),
            subject_type: request.subject_type,
            subject_id: request.subject_id,
            agent: request.agent,
            action: request.action,
            metadata: request.metadata,
            created_at: current_rfc3339(),
        }) {
            Ok(annotation) => annotation,
            Err(AnnotationError::NotFound) => {
                return problem_response(not_found_problem(not_found_detail));
            }
            Err(AnnotationError::InvalidSubjectType) => {
                return problem_response(invalid_annotation_subject_type_problem());
            }
            Err(AnnotationError::InvalidLimit | AnnotationError::InvalidCursor) => {
                return problem_response(internal_problem());
            }
            Err(AnnotationError::Sqlite(_)) => return problem_response(internal_problem()),
        }
    };

    let timestamp = annotation.created_at.clone();
    let payload = json!({
        "event": "annotation.added",
        "annotation": annotation,
        "timestamp": timestamp,
    });
    let _ = state.effect_sink.handle(&[IngestEffect::EnqueueWebhook {
        event: "annotation.added".to_owned(),
        payload_json: payload.to_string(),
    }]);

    json_status_response(StatusCode::CREATED.as_u16(), annotation)
}

fn parse_annotation_create(
    attrs: Map<String, Value>,
    fixed_subject: Option<(&'static str, String)>,
) -> Result<AnnotationCreate, Box<ProblemDetails>> {
    let required_fields = if fixed_subject.is_some() {
        &["agent", "action"][..]
    } else {
        &["subject_type", "subject_id", "agent", "action"][..]
    };
    if annotation_has_invalid_required_type(&attrs, required_fields) {
        return Err(Box::new(invalid_annotation_problem()));
    }

    let mut errors: ValidationErrors = ValidationErrors::new();
    let unified_route = fixed_subject.is_none();
    let (subject_type, subject_id) = match fixed_subject {
        Some((subject_type, subject_id)) => (subject_type.to_owned(), subject_id),
        None => {
            let subject_type = required_annotation_string(&attrs, "subject_type", &mut errors);
            let subject_id = required_annotation_string(&attrs, "subject_id", &mut errors);
            (
                subject_type.unwrap_or_default(),
                subject_id.unwrap_or_default(),
            )
        }
    };
    let agent = required_annotation_string(&attrs, "agent", &mut errors);
    let action = required_annotation_string(&attrs, "action", &mut errors);
    let metadata = attrs.get("metadata").cloned();

    if !errors.is_empty() {
        return Err(Box::new(validation_problem(
            "Missing required fields.",
            errors,
        )));
    }
    if unified_route
        && !canary_store::annotation_subject_types()
            .iter()
            .any(|allowed| *allowed == subject_type)
    {
        return Err(Box::new(invalid_annotation_subject_type_problem()));
    }

    Ok(AnnotationCreate {
        subject_type,
        subject_id,
        agent: agent.unwrap_or_default(),
        action: action.unwrap_or_default(),
        metadata,
    })
}

fn annotation_has_invalid_required_type(attrs: &Map<String, Value>, fields: &[&str]) -> bool {
    fields.iter().any(|field| {
        attrs
            .get(*field)
            .is_some_and(|value| !value.is_string() && !value.is_null())
    })
}

fn required_annotation_string(
    attrs: &Map<String, Value>,
    field: &str,
    errors: &mut ValidationErrors,
) -> Option<String> {
    match attrs.get(field) {
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(Value::String(_)) | None | Some(Value::Null) => {
            errors.insert(field.to_owned(), vec!["is required".to_owned()]);
            None
        }
        Some(_) => {
            errors.insert(field.to_owned(), vec!["must be a string".to_owned()]);
            None
        }
    }
}
