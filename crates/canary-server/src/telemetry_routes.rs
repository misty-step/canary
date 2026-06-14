//! Axum adapter for bounded analytics event ingestion.

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::{
    problem_details::{internal_problem, payload_too_large_problem, validation_problem},
    request::{MAX_JSON_BODY_BYTES, decode_json_object},
};
use canary_ingest::{IngestEffect, ValidationErrors};
use canary_store::{TelemetryEventError, TelemetryEventInsert};
use serde_json::{Map, Value};

use crate::{
    IngestState,
    body_fields::{optional_string, required_string},
    enforce_service_authority,
    http_contract::{check_content_length, json_status_response, problem_response},
    require_ingest_scope,
    server_time::current_rfc3339,
};

pub(crate) async fn create_event(
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

    let key = match require_ingest_scope(&state, &headers) {
        Ok(key) => key,
        Err(problem) => return problem_response(*problem),
    };
    let attrs = match decode_json_object(&body, None) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let insert = match parse_event(attrs, &key.tenant_id, &key.project_id) {
        Ok(insert) => insert,
        Err(errors) => {
            return problem_response(validation_problem("Invalid event payload.", errors));
        }
    };
    if let Err(problem) = enforce_service_authority(&key, &insert.service) {
        return problem_response(*problem);
    }
    let tenant_id = key.tenant_id.clone();
    let project_id = key.project_id.clone();

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    let result = store.insert_telemetry_event(insert);
    drop(store);

    match result {
        Ok(event) => {
            let payload_json = webhook_payload(&event, &tenant_id, &project_id);
            let _ = state.handle_effects(&[IngestEffect::EnqueueWebhook {
                event: event.event.clone(),
                payload_json,
            }]);
            json_status_response(StatusCode::CREATED.as_u16(), event)
        }
        Err(TelemetryEventError::Validation(errors)) => {
            let mut validation = ValidationErrors::new();
            for (field, message) in errors {
                validation.insert(field.to_owned(), vec![message.to_owned()]);
            }
            problem_response(validation_problem("Invalid event payload.", validation))
        }
        Err(TelemetryEventError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

fn webhook_payload(
    event: &canary_core::query::TelemetryEvent,
    tenant_id: &str,
    project_id: &str,
) -> String {
    let mut payload = serde_json::to_value(event).unwrap_or_else(|_| Value::Object(Map::new()));
    if let Some(object) = payload.as_object_mut() {
        object.insert("tenant_id".to_owned(), Value::String(tenant_id.to_owned()));
        object.insert(
            "project_id".to_owned(),
            Value::String(project_id.to_owned()),
        );
    }
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned())
}

fn parse_event(
    attrs: Map<String, Value>,
    tenant_id: &str,
    project_id: &str,
) -> Result<TelemetryEventInsert, ValidationErrors> {
    let mut errors = ValidationErrors::new();
    let service = required_string(&attrs, "service", &mut errors);
    let name = required_string(&attrs, "name", &mut errors);
    let summary = required_string(&attrs, "summary", &mut errors);
    let severity = optional_string(attrs.get("severity")).unwrap_or_else(|| "info".to_owned());
    let retention_class =
        optional_string(attrs.get("retention_class")).unwrap_or_else(|| "standard".to_owned());
    let privacy_policy =
        optional_string(attrs.get("privacy_policy")).unwrap_or_else(|| "redacted".to_owned());
    let sampling_policy =
        optional_string(attrs.get("sampling_policy")).unwrap_or_else(|| "unsampled".to_owned());
    let attributes = attrs
        .get("attributes")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    if !attributes.is_object() {
        errors.insert(
            "attributes".to_owned(),
            vec!["must be an object".to_owned()],
        );
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(TelemetryEventInsert {
        id: canary_core::ids::EventId::generate(),
        tenant_id: tenant_id.to_owned(),
        project_id: project_id.to_owned(),
        service: service.unwrap_or_default(),
        name: name.unwrap_or_default(),
        severity,
        summary: summary.unwrap_or_default(),
        attributes_json: attributes.to_string(),
        retention_class,
        privacy_policy,
        sampling_policy,
        created_at: current_rfc3339(),
    })
}
