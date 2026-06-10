//! Axum adapters for Canary's ingest routes.
//!
//! `POST /api/v1/errors` and `POST /api/v1/check-ins` are write surfaces with
//! different domain backends but the same HTTP boundary: size preflight,
//! ingest-scope auth, JSON object decoding, deterministic Problem Details, and
//! best-effort fanout after the SQLite commit.

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::{
    problem_details::{
        ProblemDetails, internal_problem, invalid_observed_at_problem, not_found_problem,
        payload_too_large_problem, validation_problem,
    },
    request::{MAX_JSON_BODY_BYTES, decode_json_object},
};
use canary_ingest::{IngestContext, IngestError, ValidationErrors, ingest as ingest_error};
use canary_store::MonitorCheckInSnapshot;
use canary_workers::health::{
    HealthPlanError, MonitorCheckInInput, MonitorCheckInStatus, MonitorMode, MonitorSnapshot,
    ObservationContext, plan_monitor_check_in,
};
use serde_json::{Map, Value, json};

use crate::{
    HealthEventSource, IngestState,
    body_fields::{optional_string, required_string},
    http_contract::{check_content_length, json_status_response, problem_response},
    require_ingest_scope,
    server_time::{current_rfc3339, current_unix_millis},
};

struct ParsedCheckIn {
    monitor_name: String,
    observed_at: String,
    input: MonitorCheckInInput,
}

pub(crate) async fn create_error(
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

    if let Err(problem) = require_ingest_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    let result = ingest_error(&mut store, &attrs, state.config(), IngestContext::now());
    drop(store);

    match result {
        Ok(accepted) => {
            let _ = state.handle_effects(&accepted.post_commit_effects);
            json_status_response(
                StatusCode::CREATED.as_u16(),
                json!({
                    "id": accepted.id,
                    "group_hash": accepted.group_hash,
                    "is_new_class": accepted.is_new_class
                }),
            )
        }
        Err(IngestError::Validation(errors)) => problem_response(validation_problem(
            "Request body has invalid fields.",
            errors,
        )),
        Err(IngestError::PayloadTooLarge(detail)) => {
            problem_response(payload_too_large_problem(detail))
        }
        Err(IngestError::Store(_)) => problem_response(internal_problem()),
    }
}

pub(crate) async fn create_check_in(
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

    if let Err(problem) = require_ingest_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let check_in = match parse_check_in(attrs) {
        Ok(check_in) => check_in,
        Err(problem) => return problem_response(*problem),
    };

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    let snapshot = match store.monitor_check_in_snapshot_by_name(&check_in.monitor_name) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return problem_response(not_found_problem("Monitor not found.")),
        Err(_) => return problem_response(internal_problem()),
    };
    let monitor = match monitor_snapshot(snapshot) {
        Ok(monitor) => monitor,
        Err(_) => return problem_response(internal_problem()),
    };
    let context = ObservationContext {
        now: check_in.observed_at.clone(),
        now_millis: current_unix_millis(),
        event_id: canary_core::ids::EventId::generate(),
        incident_id: canary_core::ids::IncidentId::generate(),
        incident_event_id: canary_core::ids::EventId::generate(),
    };
    let plan = match plan_monitor_check_in(monitor, check_in.input, context) {
        Ok(plan) => plan,
        Err(HealthPlanError::InvalidObservedAt(_)) => {
            return problem_response(invalid_observed_at_problem());
        }
    };
    let response_observed_at = plan.commit.check_in.observed_at.clone();
    let response_check_in_id = plan.commit.check_in.id.clone();
    let response_monitor_id = plan.commit.monitor_id.clone();
    let response_state = plan.commit.state.clone();
    let commit = match store.commit_monitor_check_in(plan.commit) {
        Ok(commit) => commit,
        Err(_) => return problem_response(internal_problem()),
    };
    drop(store);

    if let Some(transition) = commit.transition {
        let _fanout_report = state
            .health_fanout()
            .dispatch(HealthEventSource::MonitorCheckIn, &transition);
    }

    json_status_response(
        StatusCode::CREATED.as_u16(),
        json!({
            "monitor_id": response_monitor_id,
            "check_in_id": response_check_in_id,
            "state": response_state,
            "observed_at": response_observed_at,
            "sequence": commit.sequence,
        }),
    )
}

fn parse_check_in(attrs: Map<String, Value>) -> Result<ParsedCheckIn, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = ValidationErrors::new();
    let monitor_name = required_string(&attrs, "monitor", &mut errors);
    let status = parse_check_in_status(attrs.get("status"), &mut errors);

    if !errors.is_empty() {
        return Err(Box::new(validation_problem(
            "Invalid check-in payload.",
            errors,
        )));
    }

    let Some(monitor_name) = monitor_name else {
        return Err(Box::new(validation_problem(
            "Invalid check-in payload.",
            ValidationErrors::new(),
        )));
    };
    let Some(status) = status else {
        return Err(Box::new(validation_problem(
            "Invalid check-in payload.",
            ValidationErrors::new(),
        )));
    };
    let observed_at = match attrs.get("observed_at") {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Null) | None => current_rfc3339(),
        Some(_) => return Err(Box::new(invalid_observed_at_problem())),
    };

    Ok(ParsedCheckIn {
        monitor_name,
        observed_at: observed_at.clone(),
        input: MonitorCheckInInput {
            id: canary_core::ids::CheckInId::generate().into_string(),
            external_id: optional_string(attrs.get("check_in_id")),
            status,
            observed_at,
            ttl_ms: positive_i64(attrs.get("ttl_ms")),
            summary: optional_string(attrs.get("summary")),
            context: encode_context(attrs.get("context")),
        },
    })
}

fn monitor_snapshot(snapshot: MonitorCheckInSnapshot) -> Result<MonitorSnapshot, String> {
    Ok(MonitorSnapshot {
        id: snapshot.id,
        name: snapshot.name,
        service: snapshot.service,
        mode: monitor_mode(&snapshot.mode)?,
        expected_every_ms: snapshot.expected_every_ms,
        grace_ms: snapshot.grace_ms,
        state: health_state(&snapshot.state)?,
    })
}

fn monitor_mode(value: &str) -> Result<MonitorMode, String> {
    match value {
        "schedule" => Ok(MonitorMode::Schedule),
        "ttl" => Ok(MonitorMode::Ttl),
        _ => Err(format!("unknown monitor mode: {value}")),
    }
}

fn health_state(value: &str) -> Result<canary_core::health::state_machine::HealthState, String> {
    canary_core::health::state_machine::HealthState::parse_persisted(value)
        .ok_or_else(|| format!("unknown health state: {value}"))
}

fn parse_check_in_status(
    value: Option<&Value>,
    errors: &mut ValidationErrors,
) -> Option<MonitorCheckInStatus> {
    let status = match value {
        Some(Value::String(value)) => match value.as_str() {
            "alive" => Some(MonitorCheckInStatus::Alive),
            "in_progress" => Some(MonitorCheckInStatus::InProgress),
            "ok" => Some(MonitorCheckInStatus::Ok),
            "error" => Some(MonitorCheckInStatus::Error),
            _ => None,
        },
        _ => None,
    };

    if status.is_none() {
        errors.insert(
            "status".to_owned(),
            vec!["must be one of: alive, in_progress, ok, error".to_owned()],
        );
    }

    status
}

fn positive_i64(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(number)) => number.as_i64().filter(|value| *value > 0),
        Some(Value::String(value)) => value.parse::<i64>().ok().filter(|value| *value > 0),
        _ => None,
    }
}

fn encode_context(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Object(_)) => value.and_then(|value| serde_json::to_string(value).ok()),
        _ => None,
    }
}
