//! Axum adapter for Canary's admin API key routes.
//!
//! Service onboarding still owns its target-and-key transaction. This module
//! owns only the direct admin key lifecycle surface.

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
use canary_store::{ApiKeyInsert, ApiKeyRecord};
use serde_json::{Map, Value, json};

use crate::{
    IngestState,
    http_contract::{check_content_length, json_status_response, problem_response},
    require_scope,
    server_time::current_rfc3339,
};

struct ApiKeyCreate {
    name: String,
    scope: String,
    service: Option<String>,
    allow_unbound: bool,
}

pub(crate) async fn list_api_keys(
    State(state): State<IngestState>,
    headers: HeaderMap,
) -> Response<Body> {
    let authority = match require_scope(&state, &headers, Permission::Admin) {
        Ok(authority) => authority,
        Err(problem) => return problem_response(*problem),
    };

    let reader = match state.read_source() {
        Ok(reader) => reader,
        Err(_) => return problem_response(internal_problem()),
    };

    match reader.list_api_keys_scoped(&authority.tenant_id, &authority.project_id) {
        Ok(keys) => json_status_response(
            StatusCode::OK.as_u16(),
            json!({"keys": keys.into_iter().map(api_key_response).collect::<Vec<_>>()}),
        ),
        Err(_) => problem_response(internal_problem()),
    }
}

pub(crate) async fn create_api_key(
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

    let authority = match require_scope(&state, &headers, Permission::Admin) {
        Ok(authority) => authority,
        Err(problem) => return problem_response(*problem),
    };

    let attrs = match decode_optional_json_object(&body) {
        Ok(attrs) => attrs,
        Err(problem) => return problem_response(*problem),
    };
    let request = match parse_api_key_create(attrs) {
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
    let key = ApiKeyInsert {
        id: canary_core::ids::ApiKeyId::generate().into_string(),
        name: request.name,
        key_prefix: raw_key
            .chars()
            .take(canary_store::API_KEY_PREFIX_LEN)
            .collect(),
        key_hash,
        created_at: current_rfc3339(),
        revoked_at: None,
        scope: request.scope,
        tenant_id: authority.tenant_id,
        project_id: authority.project_id,
        service: request.service,
        allow_unbound: request.allow_unbound,
    };
    let response_body = api_key_insert_response(&key, &raw_key);

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.insert_api_key(key) {
        Ok(()) => json_status_response(StatusCode::CREATED.as_u16(), response_body),
        Err(_) => problem_response(internal_problem()),
    }
}

pub(crate) async fn revoke_api_key(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    let authority = match require_scope(&state, &headers, Permission::Admin) {
        Ok(authority) => authority,
        Err(problem) => return problem_response(*problem),
    };

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.revoke_api_key_scoped(
        &id,
        &current_rfc3339(),
        &authority.tenant_id,
        &authority.project_id,
    ) {
        Ok(true) => {
            // Revocation must beat the auth cache: drop every cached token
            // for this key so the next request re-verifies against the store.
            state.auth_cache().invalidate_key_id(&id);
            json_status_response(StatusCode::OK.as_u16(), json!({"status": "revoked"}))
        }
        Ok(false) => problem_response(not_found_problem("API key not found.")),
        Err(_) => problem_response(internal_problem()),
    }
}

fn api_key_response(key: ApiKeyRecord) -> Value {
    json!({
        "id": key.id,
        "name": key.name,
        "scope": key.scope,
        "key_prefix": key.key_prefix,
        "active": key.revoked_at.is_none(),
        "created_at": key.created_at,
        "revoked_at": key.revoked_at,
        "tenant_id": key.tenant_id,
        "project_id": key.project_id,
        "service": key.service,
        "allow_unbound": key.allow_unbound,
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
        "tenant_id": key.tenant_id,
        "project_id": key.project_id,
        "service": key.service,
        "allow_unbound": key.allow_unbound,
        "warning": "Store this key securely. It will not be shown again.",
    })
}

fn decode_optional_json_object(body: &Bytes) -> Result<Map<String, Value>, Box<ProblemDetails>> {
    if body.is_empty() {
        Ok(Map::new())
    } else {
        decode_json_object(body, None)
    }
}

fn parse_api_key_create(attrs: Map<String, Value>) -> Result<ApiKeyCreate, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = ValidationErrors::new();
    for field in attrs.keys() {
        if !matches!(
            field.as_str(),
            "name" | "scope" | "service" | "allow_unbound"
        ) {
            errors.insert(field.clone(), vec!["is not permitted".to_owned()]);
        }
    }
    let name = match attrs.get("name") {
        Some(Value::String(value)) if !value.is_empty() => value.clone(),
        Some(Value::String(_)) => {
            errors.insert("name".to_owned(), vec!["can't be blank".to_owned()]);
            "unnamed".to_owned()
        }
        Some(Value::Null) | None => "unnamed".to_owned(),
        Some(_) => {
            errors.insert("name".to_owned(), vec!["must be a string".to_owned()]);
            "unnamed".to_owned()
        }
    };
    let scope = match attrs.get("scope") {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Null) | None => "admin".to_owned(),
        Some(_) => {
            errors.insert("scope".to_owned(), vec!["must be a string".to_owned()]);
            "admin".to_owned()
        }
    };
    if !matches!(
        scope.as_str(),
        "admin" | "ingest-only" | "read-only" | "responder-write"
    ) && !matches!(attrs.get("scope"), Some(value) if !value.is_string())
    {
        errors.insert("scope".to_owned(), vec!["is invalid".to_owned()]);
    }
    let service = match attrs.get("service") {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_owned()),
        Some(Value::String(_)) => {
            errors.insert("service".to_owned(), vec!["can't be blank".to_owned()]);
            None
        }
        Some(Value::Null) | None => None,
        Some(_) => {
            errors.insert("service".to_owned(), vec!["must be a string".to_owned()]);
            None
        }
    };
    let allow_unbound = match attrs.get("allow_unbound") {
        Some(Value::Bool(value)) => *value,
        Some(Value::Null) | None => false,
        Some(_) => {
            errors.insert(
                "allow_unbound".to_owned(),
                vec!["must be a boolean".to_owned()],
            );
            false
        }
    };
    if service.is_some() && scope == "admin" {
        errors.insert(
            "service".to_owned(),
            vec!["cannot be set on admin keys".to_owned()],
        );
    }
    if service.is_none() && scope == "responder-write" {
        errors.insert(
            "service".to_owned(),
            vec!["is required for responder-write keys".to_owned()],
        );
    }
    if scope == "read-only" && service.is_none() && !allow_unbound {
        errors.insert(
            "service".to_owned(),
            vec!["is required unless allow_unbound is true".to_owned()],
        );
    }
    if service.is_some() && allow_unbound {
        errors.insert(
            "allow_unbound".to_owned(),
            vec!["cannot be combined with a service binding".to_owned()],
        );
    }
    if allow_unbound && scope != "read-only" {
        errors.insert(
            "allow_unbound".to_owned(),
            vec!["is only valid for read-only keys".to_owned()],
        );
    }

    if errors.is_empty() {
        Ok(ApiKeyCreate {
            name,
            scope,
            service,
            allow_unbound,
        })
    } else {
        Err(Box::new(validation_problem(
            "Invalid API key request.",
            errors,
        )))
    }
}
