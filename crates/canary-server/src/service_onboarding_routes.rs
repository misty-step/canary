//! Axum adapter for Canary's one-shot service onboarding route.
//!
//! Service onboarding deliberately owns its target-and-key transaction instead
//! of calling the neighboring admin target or key handlers. The route creates
//! one health target, one scoped ingest key, and one agent-facing response with
//! links/snippets that make the new service immediately replayable.

use std::collections::BTreeMap;

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, HeaderName, Response, StatusCode, header::HOST},
};
use canary_http::{
    auth::Permission,
    problem_details::{
        ProblemDetails, internal_problem, payload_too_large_problem, validation_problem,
    },
    request::{MAX_JSON_BODY_BYTES, decode_json_object},
};
use canary_ingest::ValidationErrors;
use canary_store::{ApiKeyInsert, StoreError, TargetConflict, TargetInsert};
use serde_json::{Map, Value, json};

use crate::server_time::current_rfc3339;
use crate::{
    IngestState, TargetProbeLifecycleCommand,
    http_contract::{check_content_length, json_status_response, problem_response},
    require_scope, validate_target_configuration, validate_target_probe_interval_ms,
};

pub(crate) async fn create_service_onboarding(
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
    let request = match parse_service_onboarding_create(attrs, state.allow_private_targets()) {
        Ok(request) => request,
        Err(problem) => return problem_response(*problem),
    };

    let raw_key = canary_core::secrets::api_key("live");
    let key_hash = {
        let raw_key = raw_key.clone();
        match tokio::task::spawn_blocking(move || bcrypt::hash(raw_key, bcrypt::DEFAULT_COST)).await
        {
            Ok(Ok(hash)) => hash,
            _ => return problem_response(internal_problem()),
        }
    };
    let created_at = current_rfc3339();
    let target = service_onboarding_target(&request, &created_at);
    let api_key = ApiKeyInsert {
        id: canary_core::ids::ApiKeyId::generate().into_string(),
        name: format!("{}-ingest", request.service),
        key_prefix: raw_key
            .chars()
            .take(canary_store::API_KEY_PREFIX_LEN)
            .collect(),
        key_hash,
        created_at,
        revoked_at: None,
        scope: "ingest-only".to_owned(),
    };
    let response_body =
        service_onboarding_response(&request, &target, &api_key, &raw_key, &base_url(&headers));
    let command = TargetProbeLifecycleCommand::Track {
        target_id: target.id.clone(),
        interval_ms: target.interval_ms,
    };

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.commit_service_onboarding_target_and_key(target, api_key) {
        Ok(()) => {}
        Err(StoreError::TargetConflict(conflict)) => {
            return problem_response(service_onboarding_conflict_problem(conflict));
        }
        Err(_) => return problem_response(internal_problem()),
    }
    drop(store);

    let _control_result = state.control_target(command);

    json_status_response(StatusCode::CREATED.as_u16(), response_body)
}

struct ServiceOnboardingCreate {
    service: String,
    url: String,
    environment: String,
    interval_ms: Option<i64>,
}

fn service_onboarding_response(
    request: &ServiceOnboardingCreate,
    target: &TargetInsert,
    api_key: &ApiKeyInsert,
    raw_key: &str,
    base_url: &str,
) -> Value {
    json!({
        "service": request.service,
        "api_key": api_key_insert_response(api_key, raw_key),
        "target": target_insert_response(target),
        "links": {
            "report": format!("{base_url}/api/v1/report?window=1h"),
            "service_query": format!(
                "{base_url}/api/v1/query?service={}&window=1h",
                encode_form_value(&request.service)
            ),
        },
        "snippets": {
            "error_ingest_curl": error_ingest_curl(base_url, raw_key, request),
            "report_curl": report_curl(base_url),
            "service_query_curl": service_query_curl(base_url, &request.service),
            "elixir_logger": elixir_logger_snippet(base_url, raw_key, request),
            "typescript_init": typescript_init_snippet(base_url, raw_key, request),
        },
    })
}

fn api_key_insert_response(key: &ApiKeyInsert, raw_key: &str) -> Value {
    json!({
        "id": key.id,
        "name": key.name,
        "scope": key.scope,
        "key": raw_key,
        "key_prefix": key.key_prefix,
        "created_at": key.created_at,
        "warning": "Store this key securely. It will not be shown again.",
    })
}

fn target_insert_response(target: &TargetInsert) -> Value {
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

fn error_ingest_curl(base_url: &str, raw_key: &str, request: &ServiceOnboardingCreate) -> String {
    let payload = serde_json::to_string(&json!({
        "service": request.service,
        "environment": request.environment,
        "error_class": "RuntimeError",
        "message": "canary onboarding check",
        "severity": "error",
        "context": {
            "source": "service-onboarding",
        },
    }))
    .unwrap_or_else(|_| "{}".to_owned());

    format!(
        "curl -X POST {base_url}/api/v1/errors \\\n  -H \"Authorization: Bearer {raw_key}\" \\\n  -H \"Content-Type: application/json\" \\\n  -d @- <<'JSON'\n{payload}\nJSON"
    )
}

fn report_curl(base_url: &str) -> String {
    format!(
        "curl \"{base_url}/api/v1/report?window=1h\" \\\n  -H \"Authorization: Bearer $CANARY_READ_KEY\""
    )
}

fn service_query_curl(base_url: &str, service: &str) -> String {
    format!(
        "curl \"{base_url}/api/v1/query?service={}&window=1h\" \\\n  -H \"Authorization: Bearer $CANARY_READ_KEY\"",
        encode_form_value(service)
    )
}

fn elixir_logger_snippet(
    base_url: &str,
    raw_key: &str,
    request: &ServiceOnboardingCreate,
) -> String {
    format!(
        "CanarySdk.attach(\n  endpoint: \"{base_url}\",\n  api_key: \"{raw_key}\",\n  service: \"{}\",\n  environment: \"{}\"\n)",
        request.service, request.environment
    )
}

fn typescript_init_snippet(
    base_url: &str,
    raw_key: &str,
    request: &ServiceOnboardingCreate,
) -> String {
    format!(
        "import {{ initCanary }} from \"@canary-obs/sdk\";\n\ninitCanary({{\n  endpoint: \"{base_url}\",\n  apiKey: \"{raw_key}\",\n  service: \"{}\",\n  environment: \"{}\"\n}});",
        request.service, request.environment
    )
}

fn base_url(headers: &HeaderMap) -> String {
    let scheme = headers
        .get(HeaderName::from_static("x-forwarded-proto"))
        .and_then(|value| value.to_str().ok())
        .filter(|value| matches!(*value, "http" | "https"))
        .unwrap_or("http");
    let host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .unwrap_or("localhost");

    format!("{scheme}://{host}")
}

fn encode_form_value(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                vec![
                    '%',
                    HEX[(byte >> 4) as usize] as char,
                    HEX[(byte & 0x0f) as usize] as char,
                ]
            }
        })
        .collect()
}

fn parse_service_onboarding_create(
    attrs: Map<String, Value>,
    configured_allow_private: bool,
) -> Result<ServiceOnboardingCreate, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = ValidationErrors::new();
    let service = required_trimmed_string(&attrs, "service", &mut errors);
    let url = required_trimmed_string(&attrs, "url", &mut errors);
    let environment = optional_trimmed_string(attrs.get("environment"))
        .unwrap_or_else(|| "production".to_owned());
    let interval_ms = optional_service_onboarding_interval(&attrs, &mut errors);
    let allow_private = match attrs.get("allow_private") {
        Some(Value::Bool(value)) => *value,
        Some(Value::Null) | None => false,
        Some(_) => {
            errors.insert(
                "allow_private".to_owned(),
                vec!["must be a boolean".to_owned()],
            );
            false
        }
    };

    if !errors.is_empty() {
        return Err(Box::new(validation_problem(
            "Invalid service onboarding request.",
            errors,
        )));
    }

    let Some(service) = service else {
        return Err(Box::new(validation_problem(
            "Invalid service onboarding request.",
            ValidationErrors::new(),
        )));
    };
    let Some(url) = url else {
        return Err(Box::new(validation_problem(
            "Invalid service onboarding request.",
            ValidationErrors::new(),
        )));
    };
    if let Err(reason) =
        validate_target_configuration(&url, "GET", None, configured_allow_private || allow_private)
    {
        return Err(Box::new(validation_problem(
            "Invalid service onboarding request.",
            BTreeMap::from([("url".to_owned(), vec![service_onboarding_url_error(reason)])]),
        )));
    }

    Ok(ServiceOnboardingCreate {
        service,
        url,
        environment,
        interval_ms,
    })
}

fn service_onboarding_target(request: &ServiceOnboardingCreate, created_at: &str) -> TargetInsert {
    TargetInsert {
        id: canary_core::ids::TargetId::generate().into_string(),
        url: request.url.clone(),
        name: request.service.clone(),
        service: request.service.clone(),
        method: "GET".to_owned(),
        headers: None,
        interval_ms: request.interval_ms.unwrap_or(60_000),
        timeout_ms: 10_000,
        expected_status: "200".to_owned(),
        body_contains: None,
        degraded_after: 1,
        down_after: 3,
        up_after: 1,
        active: true,
        created_at: created_at.to_owned(),
    }
}

fn optional_service_onboarding_interval(
    attrs: &Map<String, Value>,
    errors: &mut ValidationErrors,
) -> Option<i64> {
    match attrs.get("interval_ms") {
        Some(Value::Number(number)) => match number.as_i64().filter(|value| *value > 0) {
            Some(value) => match validate_target_probe_interval_ms(value) {
                Ok(()) => Some(value),
                Err(reason) => {
                    errors.insert("interval_ms".to_owned(), vec![reason]);
                    None
                }
            },
            None => {
                errors.insert(
                    "interval_ms".to_owned(),
                    vec!["must be greater than 0".to_owned()],
                );
                None
            }
        },
        Some(Value::Null) | None => None,
        Some(_) => {
            errors.insert(
                "interval_ms".to_owned(),
                vec!["must be an integer".to_owned()],
            );
            None
        }
    }
}

fn required_trimmed_string(
    attrs: &Map<String, Value>,
    key: &str,
    errors: &mut ValidationErrors,
) -> Option<String> {
    match attrs.get(key) {
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                errors.insert(key.to_owned(), vec!["can't be blank".to_owned()]);
                None
            } else {
                Some(value.to_owned())
            }
        }
        Some(_) => {
            errors.insert(key.to_owned(), vec!["must be a string".to_owned()]);
            None
        }
        None => {
            errors.insert(key.to_owned(), vec!["can't be blank".to_owned()]);
            None
        }
    }
}

fn optional_trimmed_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_owned())
            }
        }
        _ => None,
    }
}

fn service_onboarding_url_error(reason: String) -> String {
    if reason == "target URL scheme must be http or https" {
        "scheme must be http or https".to_owned()
    } else if let Some(rest) = reason.strip_prefix("invalid target URL: ") {
        rest.to_owned()
    } else {
        reason
    }
}

fn service_onboarding_conflict_problem(conflict: TargetConflict) -> ProblemDetails {
    let mut errors: ValidationErrors = ValidationErrors::new();
    if conflict.service {
        errors.insert(
            "service".to_owned(),
            vec!["already has a health target".to_owned()],
        );
    }
    if conflict.url {
        errors.insert("url".to_owned(), vec!["is already monitored".to_owned()]);
    }

    validation_problem("Invalid service onboarding request.", errors)
}
