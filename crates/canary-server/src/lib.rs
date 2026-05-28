//! Axum server wiring for Canary.
//!
//! This crate adapts the stable wire contracts from `canary-http` to concrete
//! HTTP responses. Domain decisions and body shapes stay out of the router.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{
        HeaderMap, HeaderValue, Response, StatusCode,
        header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderName},
    },
    routing::{get, post},
};
use canary_http::public::{
    DependencyStatus, PublicResponse, healthz_response, openapi_response, readyz_response,
};
use canary_http::{
    auth::{ApiKeyScope, Permission, authorize},
    problem_details::{ProblemCode, ProblemDetails},
    request::{
        MAX_JSON_BODY_BYTES, decode_json_object,
        payload_too_large_problem as http_payload_too_large_problem,
    },
};
use canary_ingest::{
    IngestConfig, IngestContext, IngestError, ValidationErrors, ingest as ingest_error,
};
use canary_store::Store;
use serde::Serialize;
use serde_json::json;

const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const PROBLEM_CONTENT_TYPE: &str = "application/problem+json; charset=utf-8";

/// Snapshot of dependency readiness for the public readiness endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicReadiness {
    database: DependencyStatus,
    supervisor: DependencyStatus,
}

impl PublicReadiness {
    /// Build readiness from explicit dependency statuses.
    pub const fn new(database: DependencyStatus, supervisor: DependencyStatus) -> Self {
        Self {
            database,
            supervisor,
        }
    }

    /// Convenience constructor for a fully ready process.
    pub const fn ready() -> Self {
        Self::new(DependencyStatus::Ok, DependencyStatus::Ok)
    }
}

/// Router for Canary's public unauthenticated endpoints.
pub fn public_router(readiness: PublicReadiness) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/v1/openapi.json", get(openapi))
        .with_state(readiness)
}

/// Router for Canary's authenticated ingest endpoints.
pub fn ingest_router(state: IngestState) -> Router {
    Router::new()
        .route("/api/v1/errors", post(create_error))
        .with_state(state)
}

/// Shared state needed by authenticated ingest routes.
#[derive(Clone)]
pub struct IngestState {
    store: Arc<Mutex<Store>>,
    api_keys: Arc<HashMap<String, ApiKeyScope>>,
    config: IngestConfig,
}

impl IngestState {
    /// Build ingest state from an already-open single-writer store.
    pub fn new(store: Store, api_keys: HashMap<String, ApiKeyScope>, config: IngestConfig) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            api_keys: Arc::new(api_keys),
            config,
        }
    }
}

async fn healthz() -> Response<Body> {
    json_response(healthz_response())
}

async fn readyz(State(readiness): State<PublicReadiness>) -> Response<Body> {
    json_response(readyz_response(readiness.database, readiness.supervisor))
}

async fn openapi() -> Response<Body> {
    text_response(openapi_response())
}

async fn create_error(
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

    let mut store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => {
            return problem_response(ProblemDetails::new(
                500,
                ProblemCode::InternalError,
                "An unexpected error occurred.",
                None,
            ));
        }
    };

    match ingest_error(&mut store, &attrs, &state.config, IngestContext::now()) {
        Ok(accepted) => json_status_response(
            StatusCode::CREATED.as_u16(),
            json!({
                "id": accepted.id,
                "group_hash": accepted.group_hash,
                "is_new_class": accepted.is_new_class
            }),
        ),
        Err(IngestError::Validation(errors)) => problem_response(validation_problem(errors)),
        Err(IngestError::PayloadTooLarge(detail)) => {
            problem_response(payload_too_large_problem(detail))
        }
        Err(IngestError::Store(_)) => problem_response(ProblemDetails::new(
            500,
            ProblemCode::InternalError,
            "An unexpected error occurred.",
            None,
        )),
    }
}

fn require_ingest_scope(
    state: &IngestState,
    headers: &HeaderMap,
) -> Result<(), Box<ProblemDetails>> {
    let authorization_headers = headers
        .get_all(AUTHORIZATION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();

    authorize(
        &authorization_headers,
        Permission::Ingest,
        |token| state.api_keys.get(token).copied(),
        None,
    )
}

fn check_content_length(headers: &HeaderMap) -> Result<(), Box<ProblemDetails>> {
    let Some(value) = headers.get(CONTENT_LENGTH) else {
        return Ok(());
    };

    let length = value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);

    if length > MAX_JSON_BODY_BYTES {
        Err(Box::new(payload_too_large_problem(
            "Request body exceeds 100KB limit.",
        )))
    } else {
        Ok(())
    }
}

fn json_response<T>(contract: PublicResponse<T>) -> Response<Body>
where
    T: Serialize,
{
    match serde_json::to_vec(&contract.body) {
        Ok(body) => response(contract.status, contract.content_type, Body::from(body)),
        Err(_) => internal_server_error(),
    }
}

fn json_status_response<T>(status: u16, body: T) -> Response<Body>
where
    T: Serialize,
{
    match serde_json::to_vec(&body) {
        Ok(body) => response(status, JSON_CONTENT_TYPE, Body::from(body)),
        Err(_) => internal_server_error(),
    }
}

fn problem_response(problem: ProblemDetails) -> Response<Body> {
    let status = problem.status;
    match serde_json::to_vec(&problem) {
        Ok(body) => response(status, PROBLEM_CONTENT_TYPE, Body::from(body)),
        Err(_) => internal_server_error(),
    }
}

fn validation_problem(errors: ValidationErrors) -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Request body has invalid fields.",
        None,
    )
    .with_extra("errors", json!(errors))
}

fn payload_too_large_problem(detail: impl Into<String>) -> ProblemDetails {
    http_payload_too_large_problem(detail, None)
}

fn text_response(contract: PublicResponse<&'static str>) -> Response<Body> {
    response(
        contract.status,
        contract.content_type,
        Body::from(contract.body),
    )
}

fn response(status: u16, content_type: &'static str, body: Body) -> Response<Body> {
    let mut response = Response::new(body);
    *response.status_mut() = status_code(status);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

fn internal_server_error() -> Response<Body> {
    response(
        StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        "text/plain; charset=utf-8",
        Body::from("internal server error"),
    )
}

fn status_code(status: u16) -> StatusCode {
    match StatusCode::from_u16(status) {
        Ok(status) => status,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Headers set by the public adapter.
pub const PUBLIC_CONTENT_TYPE: HeaderName = CONTENT_TYPE;

#[cfg(test)]
mod tests {
    use std::error::Error;

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header::CONTENT_TYPE},
    };
    use canary_http::public::{APPLICATION_JSON, OPENAPI_JSON};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn healthz_adapts_the_public_contract() -> Result<(), Box<dyn Error>> {
        let response = public_router(PublicReadiness::ready())
            .oneshot(Request::get("/healthz").body(Body::empty())?)
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static(APPLICATION_JSON))
        );
        assert_eq!(json_body(response).await?, json!({"status": "ok"}));

        Ok(())
    }

    #[tokio::test]
    async fn readyz_returns_ready_when_all_dependencies_are_ok() -> Result<(), Box<dyn Error>> {
        let response = public_router(PublicReadiness::ready())
            .oneshot(Request::get("/readyz").body(Body::empty())?)
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await?,
            json!({
                "status": "ready",
                "checks": {
                    "database": "ok",
                    "supervisor": "ok"
                }
            })
        );

        Ok(())
    }

    #[tokio::test]
    async fn readyz_returns_503_when_any_dependency_fails() -> Result<(), Box<dyn Error>> {
        let cases = [
            PublicReadiness::new(DependencyStatus::Error, DependencyStatus::Ok),
            PublicReadiness::new(DependencyStatus::Ok, DependencyStatus::Error),
            PublicReadiness::new(DependencyStatus::Error, DependencyStatus::Error),
        ];

        for readiness in cases {
            let response = public_router(readiness)
                .oneshot(Request::get("/readyz").body(Body::empty())?)
                .await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["status"], "not_ready");
        }

        Ok(())
    }

    #[tokio::test]
    async fn openapi_serves_the_checked_in_document_unchanged() -> Result<(), Box<dyn Error>> {
        let response = public_router(PublicReadiness::ready())
            .oneshot(Request::get("/api/v1/openapi.json").body(Body::empty())?)
            .await?;
        let content_type = response.headers().get(CONTENT_TYPE).cloned();
        let body = to_bytes(response.into_body(), usize::MAX).await?;

        assert_eq!(
            content_type,
            Some(HeaderValue::from_static(APPLICATION_JSON))
        );
        assert_eq!(body.as_ref(), OPENAPI_JSON.as_bytes());

        Ok(())
    }

    #[tokio::test]
    async fn public_router_does_not_mount_private_routes() -> Result<(), Box<dyn Error>> {
        let response = public_router(PublicReadiness::ready())
            .oneshot(Request::get("/api/v1/query").body(Body::empty())?)
            .await?;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        Ok(())
    }

    #[tokio::test]
    async fn error_ingest_accepts_ingest_scope_and_returns_summary() -> Result<(), Box<dyn Error>> {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(error_request("sk_ingest", valid_error_body())?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::CREATED);
        assert!(body["id"].as_str().is_some_and(|id| id.starts_with("ERR-")));
        assert_eq!(body["group_hash"].as_str().map(str::len), Some(64));
        assert_eq!(body["is_new_class"], true);

        Ok(())
    }

    #[tokio::test]
    async fn error_ingest_accepts_admin_scope() -> Result<(), Box<dyn Error>> {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(error_request("sk_admin", valid_error_body())?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);

        Ok(())
    }

    #[tokio::test]
    async fn error_ingest_rejects_missing_invalid_and_wrong_scope_keys()
    -> Result<(), Box<dyn Error>> {
        let cases = [
            (
                Request::post("/api/v1/errors").body(Body::from(valid_error_body()))?,
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
            ),
            (
                error_request("sk_unknown", valid_error_body())?,
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
            ),
            (
                error_request("sk_read", valid_error_body())?,
                StatusCode::FORBIDDEN,
                "insufficient_scope",
            ),
        ];

        for (request, expected_status, expected_code) in cases {
            let response = ingest_router(test_ingest_state()?).oneshot(request).await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, expected_status);
            assert_eq!(body["code"], expected_code);
        }

        Ok(())
    }

    #[tokio::test]
    async fn error_ingest_reports_validation_errors_without_writing() -> Result<(), Box<dyn Error>>
    {
        let state = test_ingest_state()?;
        let router = ingest_router(state.clone());
        assert_eq!(error_count(&state)?, 0);

        let response = router
            .clone()
            .oneshot(error_request("sk_ingest", "{}")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "validation_error");
        assert!(body["errors"].get("service").is_some());
        assert_eq!(error_count(&state)?, 0);

        let response = router
            .oneshot(error_request("sk_ingest", valid_error_body())?)
            .await?;
        let body = json_body(response).await?;
        assert_eq!(body["is_new_class"], true);
        assert_eq!(error_count(&state)?, 1);

        Ok(())
    }

    #[tokio::test]
    async fn content_length_preflight_rejects_large_payload_without_writing()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state.clone());
        assert_eq!(error_count(&state)?, 0);

        let response = router
            .clone()
            .oneshot(
                Request::post("/api/v1/errors")
                    .header("authorization", "Bearer sk_ingest")
                    .header("content-length", "102401")
                    .body(Body::from("{"))?,
            )
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body["code"], "payload_too_large");
        assert_eq!(body["detail"], "Request body exceeds 100KB limit.");
        assert_eq!(error_count(&state)?, 0);

        let response = router
            .oneshot(error_request("sk_ingest", valid_error_body())?)
            .await?;
        let body = json_body(response).await?;
        assert_eq!(body["is_new_class"], true);
        assert_eq!(error_count(&state)?, 1);

        Ok(())
    }

    #[tokio::test]
    async fn malformed_json_is_rejected_after_auth_without_writing() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state.clone());
        assert_eq!(error_count(&state)?, 0);

        let response = router
            .clone()
            .oneshot(error_request("sk_ingest", "{")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_request");
        assert_eq!(error_count(&state)?, 0);

        let response = router
            .oneshot(error_request("sk_ingest", valid_error_body())?)
            .await?;
        let body = json_body(response).await?;
        assert_eq!(body["is_new_class"], true);
        assert_eq!(error_count(&state)?, 1);

        Ok(())
    }

    #[tokio::test]
    async fn unauthorized_request_is_rejected_before_json_decode_and_without_writing()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state.clone());
        assert_eq!(error_count(&state)?, 0);

        let response = router
            .clone()
            .oneshot(Request::post("/api/v1/errors").body(Body::from("{"))?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "invalid_api_key");
        assert_eq!(error_count(&state)?, 0);

        let response = router
            .oneshot(error_request("sk_ingest", valid_error_body())?)
            .await?;
        let body = json_body(response).await?;
        assert_eq!(body["is_new_class"], true);
        assert_eq!(error_count(&state)?, 1);

        Ok(())
    }

    async fn json_body(response: Response<Body>) -> Result<Value, Box<dyn Error>> {
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice(&bytes)?;

        Ok(body)
    }

    fn test_ingest_state() -> Result<IngestState, Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;

        Ok(IngestState::new(
            store,
            HashMap::from([
                ("sk_admin".to_owned(), ApiKeyScope::Admin),
                ("sk_ingest".to_owned(), ApiKeyScope::IngestOnly),
                ("sk_read".to_owned(), ApiKeyScope::ReadOnly),
            ]),
            IngestConfig::default(),
        ))
    }

    fn error_request(token: &str, body: &'static str) -> Result<Request<Body>, Box<dyn Error>> {
        Ok(Request::post("/api/v1/errors")
            .header("authorization", format!("Bearer {token}"))
            .header(CONTENT_TYPE, APPLICATION_JSON)
            .body(Body::from(body))?)
    }

    fn error_count(state: &IngestState) -> Result<u64, Box<dyn Error>> {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        Ok(store.error_count()?)
    }

    fn valid_error_body() -> &'static str {
        r#"{"service":"test-svc","error_class":"RuntimeError","message":"something went wrong"}"#
    }
}
