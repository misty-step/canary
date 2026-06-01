//! Axum adapter for Canary's admin monitor mutation routes.
//!
//! Monitor check-ins and overdue runtime stay outside this module. This adapter
//! owns only the admin surface for configuring non-HTTP monitor definitions.

use std::collections::BTreeMap;

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::{
    auth::Permission,
    problem_details::{
        ProblemDetails, internal_problem, not_found_problem, payload_too_large_problem,
        validation_problem,
    },
    request::{MAX_JSON_BODY_BYTES, decode_json_object},
};
use canary_ingest::ValidationErrors;
use canary_store::{MonitorInsert, MonitorRecord};
use serde_json::{Map, Value, json};

use crate::{
    IngestState, check_content_length, current_rfc3339, json_status_response,
    optional_positive_i64, optional_string, problem_response, require_scope, required_string,
    response,
};

pub(crate) async fn list_monitors(
    State(state): State<IngestState>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.list_monitors() {
        Ok(monitors) => json_status_response(
            StatusCode::OK.as_u16(),
            json!({"monitors": monitors.into_iter().map(monitor_response).collect::<Vec<_>>()}),
        ),
        Err(_) => problem_response(internal_problem()),
    }
}

pub(crate) async fn create_monitor(
    State(state): State<IngestState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if let Err(problem) = check_content_length(&headers) {
        return problem_response(*problem);
    }

    if body.len() as u64 > MAX_JSON_BODY_BYTES {
        return problem_response(payload_too_large_problem(
            "Request body exceeds 100KB limit.",
        ));
    }

    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }

    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let monitor = match parse_monitor_create(attrs) {
        Ok(monitor) => monitor,
        Err(problem) => return problem_response(*problem),
    };
    let response_body = monitor_insert_response(&monitor);

    let mut store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.create_monitor(monitor) {
        Ok(true) => json_status_response(StatusCode::CREATED.as_u16(), response_body),
        Ok(false) => problem_response(validation_problem(
            "Invalid monitor configuration.",
            BTreeMap::from([("name".to_owned(), vec!["has already been taken".to_owned()])]),
        )),
        Err(_) => problem_response(internal_problem()),
    }
}

pub(crate) async fn delete_monitor(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }

    let mut store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.delete_monitor(&id) {
        Ok(true) => response(
            StatusCode::NO_CONTENT.as_u16(),
            "text/plain; charset=utf-8",
            Body::empty(),
        ),
        Ok(false) => problem_response(not_found_problem("Monitor not found.")),
        Err(_) => problem_response(internal_problem()),
    }
}

fn monitor_response(monitor: MonitorRecord) -> Value {
    json!({
        "id": monitor.id,
        "name": monitor.name,
        "service": monitor.service,
        "mode": monitor.mode,
        "expected_every_ms": monitor.expected_every_ms,
        "grace_ms": monitor.grace_ms,
        "created_at": monitor.created_at,
    })
}

fn monitor_insert_response(monitor: &MonitorInsert) -> Value {
    json!({
        "id": monitor.id,
        "name": monitor.name,
        "service": monitor.service,
        "mode": monitor.mode,
        "expected_every_ms": monitor.expected_every_ms,
        "grace_ms": monitor.grace_ms,
        "created_at": monitor.created_at,
    })
}

fn parse_monitor_create(attrs: Map<String, Value>) -> Result<MonitorInsert, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = ValidationErrors::new();
    let name = required_string(&attrs, "name", &mut errors);
    let mode = required_string(&attrs, "mode", &mut errors);
    if let Some(mode) = mode.as_deref()
        && !matches!(mode, "schedule" | "ttl")
    {
        errors.insert(
            "mode".to_owned(),
            vec!["must be one of: schedule, ttl".to_owned()],
        );
    }
    let expected_every_ms = optional_positive_i64(&attrs, "expected_every_ms", 0, &mut errors);
    if !attrs.contains_key("expected_every_ms") {
        errors.insert(
            "expected_every_ms".to_owned(),
            vec!["must be a positive integer".to_owned()],
        );
    }
    let grace_ms = optional_non_negative_i64(&attrs, "grace_ms", 0, &mut errors);

    if !errors.is_empty() {
        return Err(Box::new(validation_problem(
            "Invalid monitor configuration.",
            errors,
        )));
    }

    let Some(name) = name else {
        return Err(Box::new(validation_problem(
            "Invalid monitor configuration.",
            ValidationErrors::new(),
        )));
    };
    let Some(mode) = mode else {
        return Err(Box::new(validation_problem(
            "Invalid monitor configuration.",
            ValidationErrors::new(),
        )));
    };

    Ok(MonitorInsert {
        id: canary_core::ids::MonitorId::generate().into_string(),
        service: optional_string(attrs.get("service")).unwrap_or_else(|| name.clone()),
        name,
        mode,
        expected_every_ms,
        grace_ms,
        created_at: current_rfc3339(),
    })
}

fn optional_non_negative_i64(
    attrs: &Map<String, Value>,
    key: &str,
    default: i64,
    errors: &mut ValidationErrors,
) -> i64 {
    match attrs.get(key) {
        Some(Value::Number(number)) => match number.as_i64().filter(|value| *value >= 0) {
            Some(value) => value,
            None => {
                errors.insert(
                    key.to_owned(),
                    vec!["must be greater than or equal to 0".to_owned()],
                );
                default
            }
        },
        Some(Value::Null) | None => default,
        Some(_) => {
            errors.insert(key.to_owned(), vec!["must be an integer".to_owned()]);
            default
        }
    }
}
