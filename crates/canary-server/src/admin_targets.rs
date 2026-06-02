//! Axum adapter for Canary's admin target mutation routes.
//!
//! The target probe lifecycle is a narrow side-effect boundary: this module
//! writes target rows through the store, then emits typed lifecycle commands.
//! Service onboarding remains outside this module because it atomically creates
//! both a target and an API key.

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::{
    auth::Permission,
    problem_details::{
        ProblemDetails, internal_problem, invalid_target_url_problem, not_found_problem,
        payload_too_large_problem, validation_problem,
    },
    request::{MAX_JSON_BODY_BYTES, decode_json_object},
};
use canary_ingest::ValidationErrors;
use canary_store::{TargetInsert, TargetRecord};
use serde_json::{Map, Value, json};

use crate::{
    IngestState, TargetProbeLifecycleCommand,
    body_fields::{
        optional_bool, optional_positive_i64, optional_positive_u32, optional_string,
        required_string,
    },
    http_contract::{check_content_length, json_status_response, problem_response, response},
    require_scope,
    server_time::current_rfc3339,
    validate_target_configuration,
};

pub(crate) async fn list_targets(
    State(state): State<IngestState>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }

    let store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.list_targets() {
        Ok(targets) => json_status_response(
            StatusCode::OK.as_u16(),
            json!({"targets": targets.into_iter().map(target_response).collect::<Vec<_>>()}),
        ),
        Err(_) => problem_response(internal_problem()),
    }
}

pub(crate) async fn create_target(
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
    let target = match parse_target_create(attrs, state.allow_private_targets()) {
        Ok(target) => target,
        Err(problem) => return problem_response(*problem),
    };
    let command = TargetProbeLifecycleCommand::Track {
        target_id: target.id.clone(),
        interval_ms: target.interval_ms,
    };
    let response_body = target_insert_response(&target);

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    if store.insert_target(target).is_err() {
        return problem_response(internal_problem());
    }
    drop(store);

    let _control_result = state.control_target(command);

    json_status_response(StatusCode::CREATED.as_u16(), response_body)
}

pub(crate) async fn delete_target(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.delete_target(&id) {
        Ok(true) => {}
        Ok(false) => return problem_response(not_found_problem("Target not found.")),
        Err(_) => return problem_response(internal_problem()),
    }
    drop(store);

    let _control_result =
        state.control_target(TargetProbeLifecycleCommand::Untrack { target_id: id });

    response(
        StatusCode::NO_CONTENT.as_u16(),
        "text/plain; charset=utf-8",
        Body::empty(),
    )
}

pub(crate) async fn update_target_interval(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
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
    let interval_ms = match parse_target_interval_update(&attrs) {
        Ok(interval_ms) => interval_ms,
        Err(problem) => return problem_response(*problem),
    };

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    let update = match store.update_target_interval(&id, interval_ms) {
        Ok(Some(update)) => update,
        Ok(None) => return problem_response(not_found_problem("Target not found.")),
        Err(_) => return problem_response(internal_problem()),
    };
    drop(store);

    if update.prior_active && update.prior_interval_ms != update.target.interval_ms {
        let _control_result = state.control_target(TargetProbeLifecycleCommand::Reconfigure {
            target_id: id,
            interval_ms: update.target.interval_ms,
        });
    }

    json_status_response(StatusCode::OK.as_u16(), target_response(update.target))
}

pub(crate) async fn pause_target(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    set_target_active(state, headers, id, false).await
}

pub(crate) async fn resume_target(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    set_target_active(state, headers, id, true).await
}

async fn set_target_active(
    state: IngestState,
    headers: HeaderMap,
    id: String,
    active: bool,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.update_target_active(&id, active) {
        Ok(true) => {}
        Ok(false) => return problem_response(not_found_problem("Target not found.")),
        Err(_) => return problem_response(internal_problem()),
    }
    drop(store);

    let command = if active {
        TargetProbeLifecycleCommand::Resume {
            target_id: id.clone(),
        }
    } else {
        TargetProbeLifecycleCommand::Pause {
            target_id: id.clone(),
        }
    };
    let _control_result = state.control_target(command);

    json_status_response(
        StatusCode::OK.as_u16(),
        json!({"status": if active { "resumed" } else { "paused" }}),
    )
}

pub(crate) fn target_insert_response(target: &TargetInsert) -> Value {
    json!({
        "id": target.id,
        "name": target.name,
        "service": target.service,
        "url": target.url,
        "method": target.method,
        "interval_ms": target.interval_ms,
        "timeout_ms": target.timeout_ms,
        "expected_status": target.expected_status,
        "active": target.active,
        "created_at": target.created_at,
    })
}

fn target_response(target: TargetRecord) -> Value {
    json!({
        "id": target.id,
        "name": target.name,
        "service": target.service,
        "url": target.url,
        "method": target.method,
        "interval_ms": target.interval_ms,
        "timeout_ms": target.timeout_ms,
        "expected_status": target.expected_status,
        "active": target.active,
        "created_at": target.created_at,
    })
}

fn parse_target_create(
    attrs: Map<String, Value>,
    configured_allow_private: bool,
) -> Result<TargetInsert, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = ValidationErrors::new();
    let name = required_string(&attrs, "name", &mut errors);
    let url = required_string(&attrs, "url", &mut errors);
    let method = optional_string(attrs.get("method")).unwrap_or_else(|| "GET".to_owned());
    if !matches!(method.as_str(), "GET" | "HEAD") {
        errors.insert(
            "method".to_owned(),
            vec!["must be one of: GET, HEAD".to_owned()],
        );
    }
    let headers = encode_target_headers(attrs.get("headers"), &mut errors);

    let interval_ms = optional_positive_i64(&attrs, "interval_ms", 60_000, &mut errors);
    let timeout_ms = optional_positive_i64(&attrs, "timeout_ms", 10_000, &mut errors);
    let degraded_after = optional_positive_u32(&attrs, "degraded_after", 1, &mut errors);
    let down_after = optional_positive_u32(&attrs, "down_after", 3, &mut errors);
    let up_after = optional_positive_u32(&attrs, "up_after", 1, &mut errors);

    if !errors.is_empty() {
        return Err(Box::new(validation_problem(
            "Invalid target configuration.",
            errors,
        )));
    }

    let Some(name) = name else {
        return Err(Box::new(validation_problem(
            "Invalid target configuration.",
            ValidationErrors::new(),
        )));
    };
    let Some(url) = url else {
        return Err(Box::new(validation_problem(
            "Invalid target configuration.",
            ValidationErrors::new(),
        )));
    };
    let service = optional_string(attrs.get("service")).unwrap_or_else(|| name.clone());
    let allow_private = configured_allow_private || optional_bool(attrs.get("allow_private"));
    if let Err(reason) =
        validate_target_configuration(&url, &method, headers.as_deref(), allow_private)
    {
        return Err(Box::new(invalid_target_url_problem(reason)));
    }

    Ok(TargetInsert {
        id: canary_core::ids::TargetId::generate().into_string(),
        url,
        name,
        service,
        method,
        headers,
        interval_ms,
        timeout_ms,
        expected_status: optional_string(attrs.get("expected_status"))
            .unwrap_or_else(|| "200".to_owned()),
        body_contains: optional_string(attrs.get("body_contains")),
        degraded_after,
        down_after,
        up_after,
        active: true,
        created_at: current_rfc3339(),
    })
}

fn parse_target_interval_update(attrs: &Map<String, Value>) -> Result<i64, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = ValidationErrors::new();
    if attrs.is_empty() {
        errors.insert(
            "interval_ms".to_owned(),
            vec!["is required for target interval updates".to_owned()],
        );
        return Err(Box::new(validation_problem(
            "Invalid target configuration.",
            errors,
        )));
    }

    for key in attrs.keys() {
        if key != "interval_ms" {
            errors.insert(
                key.clone(),
                vec!["is not supported by this endpoint".to_owned()],
            );
        }
    }

    let interval_ms = optional_positive_i64(attrs, "interval_ms", 60_000, &mut errors);
    if !attrs.contains_key("interval_ms") {
        errors.insert(
            "interval_ms".to_owned(),
            vec!["is required for target interval updates".to_owned()],
        );
    }

    if errors.is_empty() {
        Ok(interval_ms)
    } else {
        Err(Box::new(validation_problem(
            "Invalid target configuration.",
            errors,
        )))
    }
}

fn encode_target_headers(value: Option<&Value>, errors: &mut ValidationErrors) -> Option<String> {
    match value {
        Some(Value::Object(object)) => {
            for (name, value) in object {
                if !value.is_string() {
                    errors.insert(
                        format!("headers.{name}"),
                        vec!["must be a string".to_owned()],
                    );
                }
            }
            if errors.keys().any(|key| key.starts_with("headers.")) {
                None
            } else {
                serde_json::to_string(object).ok()
            }
        }
        Some(Value::Null) | None => None,
        Some(_) => {
            errors.insert(
                "headers".to_owned(),
                vec!["must be an object of string values".to_owned()],
            );
            None
        }
    }
}
