//! Axum server wiring for Canary.
//!
//! This crate adapts the stable wire contracts from `canary-http` to concrete
//! HTTP responses. Domain decisions and body shapes stay out of the router.

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
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
    auth::{ApiKeyScope, AuthError, Permission, authorize_with_lookup},
    problem_details::{ProblemCode, ProblemDetails},
    request::{
        MAX_JSON_BODY_BYTES, decode_json_object,
        payload_too_large_problem as http_payload_too_large_problem,
    },
};
use canary_ingest::{
    IngestConfig, IngestContext, IngestEffect, IngestError, ValidationErrors,
    ingest as ingest_error,
};
use canary_store::{IncidentListOptions, Store, WebhookDeliveryInsert, WebhookSubscription};
use canary_store::{QueryError, ServiceQueryOptions};
use canary_workers::webhooks::{
    WebhookEndpoint, WebhookEnqueueDecision, WebhookJob, plan_enqueue_for_event,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

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
        .route("/api/v1/query", get(query_errors))
        .route("/api/v1/incidents", get(list_incidents))
        .route("/api/v1/incidents/{id}", get(show_incident))
        .route("/api/v1/errors/{id}", get(show_error))
        .with_state(state)
}

/// Shared state needed by authenticated ingest routes.
#[derive(Clone)]
pub struct IngestState {
    store: Arc<Mutex<Store>>,
    config: IngestConfig,
    effect_sink: Arc<dyn IngestEffectSink>,
}

impl IngestState {
    /// Build ingest state from an already-open single-writer store.
    pub fn new(store: Store, config: IngestConfig) -> Self {
        Self::new_with_effect_sink(store, config, Arc::new(NoopIngestEffectSink))
    }

    /// Build ingest state with Rust webhook enqueue wired to a scheduler.
    ///
    /// This constructor persists webhook ledger rows and calls the supplied
    /// scheduler for `EnqueueWebhook` effects. It does not implement delivery
    /// transport or retry runtime; those remain behind the scheduler boundary.
    pub fn new_with_webhook_scheduler(
        store: Store,
        config: IngestConfig,
        scheduler: Arc<dyn WebhookScheduler>,
    ) -> Self {
        let store = Arc::new(Mutex::new(store));
        let effect_sink = Arc::new(WebhookEnqueueEffectSink::new(
            store.clone(),
            scheduler,
            Arc::new(NoopWebhookCooldown),
        ));
        Self {
            store,
            config,
            effect_sink,
        }
    }

    /// Build ingest state with an explicit post-commit effect sink.
    pub fn new_with_effect_sink(
        store: Store,
        config: IngestConfig,
        effect_sink: Arc<dyn IngestEffectSink>,
    ) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            config,
            effect_sink,
        }
    }
}

/// Best-effort sink for ingest effects emitted after the store transaction commits.
pub trait IngestEffectSink: Send + Sync + 'static {
    /// Handle effects. Errors are advisory and must not change the HTTP response.
    fn handle(&self, effects: &[IngestEffect]) -> Result<(), String>;
}

/// Runtime boundary for scheduling webhook delivery jobs.
pub trait WebhookScheduler: Send + Sync + 'static {
    /// Schedule one webhook job after its pending ledger row has been created.
    fn schedule(&self, job: &WebhookJob) -> Result<(), String>;
}

/// Runtime boundary for webhook cooldown state.
pub trait WebhookCooldown: Send + Sync + 'static {
    /// Return true when the event should be suppressed.
    fn in_cooldown(&self, key: &str) -> bool;

    /// Mark a key after the scheduler accepts a job.
    fn mark(&self, key: &str);
}

#[derive(Debug, Default)]
struct NoopIngestEffectSink;

impl IngestEffectSink for NoopIngestEffectSink {
    fn handle(&self, _effects: &[IngestEffect]) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Default)]
struct NoopWebhookCooldown;

impl WebhookCooldown for NoopWebhookCooldown {
    fn in_cooldown(&self, _key: &str) -> bool {
        false
    }

    fn mark(&self, _key: &str) {}
}

/// Effect sink that turns ingest webhook effects into ledger rows and jobs.
pub struct WebhookEnqueueEffectSink {
    store: Arc<Mutex<Store>>,
    scheduler: Arc<dyn WebhookScheduler>,
    cooldown: Arc<dyn WebhookCooldown>,
}

impl WebhookEnqueueEffectSink {
    /// Build a webhook enqueue sink from explicit runtime boundaries.
    pub fn new(
        store: Arc<Mutex<Store>>,
        scheduler: Arc<dyn WebhookScheduler>,
        cooldown: Arc<dyn WebhookCooldown>,
    ) -> Self {
        Self {
            store,
            scheduler,
            cooldown,
        }
    }
}

impl IngestEffectSink for WebhookEnqueueEffectSink {
    fn handle(&self, effects: &[IngestEffect]) -> Result<(), String> {
        let mut errors = Vec::new();
        for effect in effects {
            if let IngestEffect::EnqueueWebhook {
                event,
                payload_json,
            } = effect
                && let Err(error) = self.enqueue_event(event, payload_json)
            {
                errors.push(error);
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

impl WebhookEnqueueEffectSink {
    fn enqueue_event(&self, event: &str, payload_json: &str) -> Result<(), String> {
        let payload = serde_json::from_str(payload_json)
            .map_err(|error| format!("invalid webhook payload: {error}"))?;
        let now = current_rfc3339();
        let subscriptions = {
            let store = self
                .store
                .lock()
                .map_err(|_| "store lock poisoned".to_owned())?;
            store
                .active_webhook_subscriptions_for_event(event)
                .map_err(|error| error.to_string())?
        };
        let endpoints = subscriptions.into_iter().map(endpoint_from_subscription);
        let decisions = plan_enqueue_for_event(
            event,
            &payload,
            endpoints,
            || canary_core::ids::DeliveryId::generate().into_string(),
            |key| self.cooldown.in_cooldown(key),
        );

        for decision in decisions {
            match decision {
                WebhookEnqueueDecision::Schedule {
                    delivery,
                    job,
                    cooldown_key,
                } => {
                    self.create_pending(delivery, &now)?;
                    match self.scheduler.schedule(&job) {
                        Ok(()) => self.cooldown.mark(&cooldown_key),
                        Err(error) => {
                            self.discard(&job, "enqueue_failed", &now)?;
                            return Err(format!("failed to schedule webhook: {error}"));
                        }
                    }
                }
                WebhookEnqueueDecision::Suppress { delivery, reason } => {
                    self.create_suppressed(delivery, &reason, &now)?;
                }
            }
        }

        Ok(())
    }

    fn create_pending(
        &self,
        delivery: canary_workers::webhooks::PlannedWebhookDelivery,
        now: &str,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        store
            .create_pending_webhook_delivery(WebhookDeliveryInsert {
                delivery_id: delivery.delivery_id,
                webhook_id: delivery.webhook_id,
                event: delivery.event,
                now: now.to_owned(),
            })
            .map_err(|error| error.to_string())
    }

    fn create_suppressed(
        &self,
        delivery: canary_workers::webhooks::PlannedWebhookDelivery,
        reason: &str,
        now: &str,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        store
            .create_suppressed_webhook_delivery(
                WebhookDeliveryInsert {
                    delivery_id: delivery.delivery_id,
                    webhook_id: delivery.webhook_id,
                    event: delivery.event,
                    now: now.to_owned(),
                },
                reason,
            )
            .map_err(|error| error.to_string())
    }

    fn discard(&self, job: &WebhookJob, reason: &str, now: &str) -> Result<(), String> {
        let Some(delivery_id) = job.delivery_id.as_deref() else {
            return Ok(());
        };
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        store
            .mark_webhook_delivery_discarded(delivery_id, reason, now)
            .map_err(|error| error.to_string())
    }
}

fn endpoint_from_subscription(subscription: WebhookSubscription) -> WebhookEndpoint {
    WebhookEndpoint {
        id: subscription.id,
        url: subscription.url,
        secret: subscription.secret,
        active: true,
    }
}

fn current_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
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

    let result = ingest_error(&mut store, &attrs, &state.config, IngestContext::now());
    drop(store);

    match result {
        Ok(accepted) => {
            let _ = state.effect_sink.handle(&accepted.post_commit_effects);
            json_status_response(
                StatusCode::CREATED.as_u16(),
                json!({
                    "id": accepted.id,
                    "group_hash": accepted.group_hash,
                    "is_new_class": accepted.is_new_class
                }),
            )
        }
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

async fn query_errors(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<QueryParams>,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Read) {
        return problem_response(*problem);
    }

    let query_kind = match (
        params.error_class.as_deref(),
        params.service.as_deref(),
        params.group_by.as_deref(),
    ) {
        (Some(error_class), service, _) => QueryKind::ErrorClass {
            error_class: error_class.to_owned(),
            service: service.map(ToOwned::to_owned),
        },
        (None, Some(service), _) => QueryKind::Service {
            service: service.to_owned(),
        },
        (None, None, Some("error_class")) => QueryKind::ErrorClasses,
        (None, None, _) => return problem_response(missing_query_problem()),
    };

    let default_window = match &query_kind {
        QueryKind::Service { .. } => "1h",
        QueryKind::ErrorClass { .. } | QueryKind::ErrorClasses => "24h",
    };
    let window = params.window.as_deref().unwrap_or(default_window);
    let options = ServiceQueryOptions {
        cursor: params.cursor,
        with_annotation: params.with_annotation,
        without_annotation: params.without_annotation,
    };

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match query_kind {
        QueryKind::Service { service } => {
            match store.errors_by_service(&service, window, options) {
                Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
                Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
                Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
            }
        }
        QueryKind::ErrorClass {
            error_class,
            service,
        } => match store.errors_by_error_class(&error_class, window, service.as_deref(), options) {
            Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
            Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
            Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
        },
        QueryKind::ErrorClasses => match store.errors_by_class(window) {
            Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
            Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
            Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
        },
    }
}

enum QueryKind {
    Service {
        service: String,
    },
    ErrorClass {
        error_class: String,
        service: Option<String>,
    },
    ErrorClasses,
}

async fn list_incidents(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<QueryParams>,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Read) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.active_incidents(IncidentListOptions {
        with_annotation: params.with_annotation,
        without_annotation: params.without_annotation,
    }) {
        Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
        Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

fn missing_query_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Provide 'service', 'error_class', or 'group_by=error_class' parameter.",
        None,
    )
}

async fn show_error(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Read) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.error_detail(&id) {
        Ok(Some(result)) => json_status_response(StatusCode::OK.as_u16(), result),
        Ok(None) => problem_response(ProblemDetails::new(
            404,
            ProblemCode::NotFound,
            format!("Error {id} not found."),
            None,
        )),
        Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

async fn show_incident(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Read) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.incident_detail(&id) {
        Ok(Some(result)) => json_status_response(StatusCode::OK.as_u16(), result),
        Ok(None) => problem_response(ProblemDetails::new(
            404,
            ProblemCode::NotFound,
            format!("Incident {id} not found."),
            None,
        )),
        Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

fn require_ingest_scope(
    state: &IngestState,
    headers: &HeaderMap,
) -> Result<(), Box<ProblemDetails>> {
    require_scope(state, headers, Permission::Ingest)
}

fn require_scope(
    state: &IngestState,
    headers: &HeaderMap,
    permission: Permission,
) -> Result<(), Box<ProblemDetails>> {
    let authorization_headers = headers
        .get_all(AUTHORIZATION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();

    authorize_with_lookup(
        &authorization_headers,
        permission,
        |token| {
            let store = state.store.lock().map_err(|_| ())?;
            store
                .verify_api_key(token)
                .map(|verified| verified.and_then(|key| ApiKeyScope::parse(&key.scope)))
                .map_err(|_| ())
        },
        None,
    )
    .map_err(|error| match error {
        AuthError::Problem(problem) => problem,
        AuthError::Lookup(()) => Box::new(ProblemDetails::new(
            500,
            ProblemCode::InternalError,
            "An unexpected error occurred.",
            None,
        )),
    })
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

fn invalid_window_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        canary_core::query::INVALID_WINDOW_DETAIL,
        None,
    )
    .with_extra(
        "errors",
        json!({"window": [canary_core::query::INVALID_WINDOW_FIELD_ERROR]}),
    )
}

fn internal_problem() -> ProblemDetails {
    ProblemDetails::new(
        500,
        ProblemCode::InternalError,
        "An unexpected error occurred.",
        None,
    )
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

#[derive(Debug, Deserialize)]
struct QueryParams {
    service: Option<String>,
    error_class: Option<String>,
    group_by: Option<String>,
    window: Option<String>,
    cursor: Option<String>,
    with_annotation: Option<String>,
    without_annotation: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::{Arc, Mutex as StdMutex};

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header::CONTENT_TYPE},
    };
    use canary_http::public::{APPLICATION_JSON, OPENAPI_JSON};
    use canary_store::{
        API_KEY_PREFIX_LEN, ApiKeyInsert, WebhookDeliveryStatus, WebhookSubscriptionInsert,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    const ADMIN_KEY: &str = "sk_live_admin_secret";
    const INGEST_KEY: &str = "sk_live_ingest_secret";
    const READ_KEY: &str = "sk_live_read_secret";
    const REVOKED_KEY: &str = "sk_live_revoked_secret";

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
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::CREATED);
        assert!(body["id"].as_str().is_some_and(|id| id.starts_with("ERR-")));
        assert_eq!(body["group_hash"].as_str().map(str::len), Some(64));
        assert_eq!(body["is_new_class"], true);
        assert!(body.get("post_commit_effects").is_none());

        Ok(())
    }

    #[tokio::test]
    async fn error_ingest_runs_post_commit_effects_best_effort() -> Result<(), Box<dyn Error>> {
        let sink = Arc::new(RecordingFailingSink::default());
        let state = test_ingest_state_with_sink(sink.clone())?;
        let response = ingest_router(state)
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::CREATED);
        assert!(body["id"].as_str().is_some_and(|id| id.starts_with("ERR-")));

        let effects = sink.effects.lock().map_err(|_| "effect lock poisoned")?;
        assert_eq!(effects.len(), 3);
        assert!(matches!(
            effects.as_slice(),
            [
                IngestEffect::BroadcastNewError { .. },
                IngestEffect::CorrelateIncident { .. },
                IngestEffect::EnqueueWebhook { event, .. }
            ] if event == "error.new_class"
        ));

        Ok(())
    }

    #[tokio::test]
    async fn error_ingest_enqueues_webhooks_into_ledger_and_scheduler() -> Result<(), Box<dyn Error>>
    {
        let scheduler = Arc::new(RecordingScheduler::default());
        let state = test_ingest_state_with_webhook_scheduler(scheduler.clone(), true)?;
        let response = ingest_router(state.clone())
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json_body(response).await?;
        assert_eq!(body["is_new_class"], true);

        let jobs = scheduler
            .jobs
            .lock()
            .map_err(|_| "scheduler lock poisoned")?;
        assert_eq!(jobs.len(), 1);
        let job = jobs.first().ok_or("missing scheduled webhook job")?;
        assert_eq!(job.webhook_id, "WHK-test");
        assert_eq!(job.event, "error.new_class");
        let delivery_id = job
            .delivery_id
            .as_deref()
            .ok_or("missing delivery id")?
            .to_owned();
        assert!(delivery_id.starts_with("DLV-"));
        drop(jobs);

        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some(delivery_id),
            ..Default::default()
        })?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Pending);
        assert_eq!(rows[0].webhook_id, "WHK-test");
        assert_eq!(rows[0].event, "error.new_class");

        Ok(())
    }

    #[tokio::test]
    async fn webhook_scheduler_failure_discards_delivery_without_failing_ingest()
    -> Result<(), Box<dyn Error>> {
        let scheduler = Arc::new(FailingScheduler);
        let state = test_ingest_state_with_webhook_scheduler(scheduler, true)?;
        let response = ingest_router(state.clone())
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);

        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            webhook_id: Some("WHK-test".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Discarded);
        assert_eq!(rows[0].reason.as_deref(), Some("enqueue_failed"));

        Ok(())
    }

    #[tokio::test]
    async fn webhook_cooldown_suppresses_delivery_without_scheduler_job()
    -> Result<(), Box<dyn Error>> {
        let scheduler = Arc::new(RecordingScheduler::default());
        let mut state = test_ingest_state_with_webhook_scheduler(scheduler.clone(), true)?;
        let cooldown = Arc::new(AlwaysCooldown);
        state.effect_sink = Arc::new(WebhookEnqueueEffectSink::new(
            state.store.clone(),
            scheduler.clone(),
            cooldown,
        ));
        let response = ingest_router(state.clone())
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            scheduler
                .jobs
                .lock()
                .map_err(|_| "scheduler lock poisoned")?
                .len(),
            0
        );

        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            webhook_id: Some("WHK-test".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Suppressed);
        assert_eq!(rows[0].reason.as_deref(), Some("cooldown"));

        Ok(())
    }

    #[tokio::test]
    async fn error_ingest_accepts_admin_scope() -> Result<(), Box<dyn Error>> {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(error_request(ADMIN_KEY, valid_error_body())?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);

        Ok(())
    }

    #[tokio::test]
    async fn error_query_accepts_read_scope_and_returns_service_groups()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = router
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/query?service=test-svc&window=24h",
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["service"], "test-svc");
        assert_eq!(body["window"], "24h");
        assert_eq!(body["total_errors"], 1);
        assert_eq!(body["groups"][0]["error_class"], "RuntimeError");
        assert_eq!(body["groups"][0]["classification"]["category"], "unknown");

        Ok(())
    }

    #[tokio::test]
    async fn error_query_accepts_error_class_with_optional_service_filter()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state);

        let first = router
            .clone()
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        assert_eq!(first.status(), StatusCode::CREATED);

        let response = router
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/query?error_class=RuntimeError&service=test-svc",
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["error_class"], "RuntimeError");
        assert_eq!(body["window"], "24h");
        assert_eq!(body["total_errors"], 1);
        assert_eq!(body["groups"][0]["service"], "test-svc");

        Ok(())
    }

    #[tokio::test]
    async fn error_query_accepts_group_by_error_class() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state);

        for (service, error_class) in [("svc-a", "FooError"), ("svc-b", "BarError")] {
            let body = format!(
                r#"{{"service":"{service}","error_class":"{error_class}","message":"boom"}}"#
            );
            let response = router
                .clone()
                .oneshot(
                    Request::post("/api/v1/errors")
                        .header("authorization", format!("Bearer {INGEST_KEY}"))
                        .header(CONTENT_TYPE, APPLICATION_JSON)
                        .body(Body::from(body))?,
                )
                .await?;
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let response = router
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/query?group_by=error_class",
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["window"], "24h");
        assert_eq!(body["total_errors"], 2);
        assert_eq!(body["total_error_classes"], 2);
        assert_eq!(body["truncated"], false);
        let classes = body["groups"]
            .as_array()
            .ok_or("groups should be an array")?
            .iter()
            .filter_map(|group| group["error_class"].as_str())
            .collect::<Vec<_>>();
        assert!(classes.contains(&"FooError"));
        assert!(classes.contains(&"BarError"));

        Ok(())
    }

    #[tokio::test]
    async fn error_query_rejects_ingest_scope_and_invalid_params() -> Result<(), Box<dyn Error>> {
        let cases = [
            (
                read_request(INGEST_KEY, "/api/v1/query?service=test-svc")?,
                StatusCode::FORBIDDEN,
                "insufficient_scope",
            ),
            (
                read_request(READ_KEY, "/api/v1/query")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
            ),
            (
                read_request(READ_KEY, "/api/v1/query?service=test-svc&window=99h")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
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
    async fn incidents_accept_read_scope_and_return_empty_summary() -> Result<(), Box<dyn Error>> {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(read_request(READ_KEY, "/api/v1/incidents")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["summary"], "No active incidents.");
        assert_eq!(body["incidents"].as_array().map(Vec::len), Some(0));

        Ok(())
    }

    #[tokio::test]
    async fn incidents_reject_ingest_scope() -> Result<(), Box<dyn Error>> {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(read_request(INGEST_KEY, "/api/v1/incidents")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "insufficient_scope");

        Ok(())
    }

    #[tokio::test]
    async fn incident_detail_accepts_read_scope_and_reports_missing_incidents()
    -> Result<(), Box<dyn Error>> {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(read_request(READ_KEY, "/api/v1/incidents/INC-missing")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
        assert_eq!(body["detail"], "Incident INC-missing not found.");

        Ok(())
    }

    #[tokio::test]
    async fn incident_detail_rejects_ingest_scope() -> Result<(), Box<dyn Error>> {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(read_request(INGEST_KEY, "/api/v1/incidents/INC-anything")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "insufficient_scope");

        Ok(())
    }

    #[tokio::test]
    async fn error_detail_accepts_read_scope_and_reports_missing_errors()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state);

        let create_response = router
            .clone()
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        let created = json_body(create_response).await?;
        let error_id = created["id"].as_str().ok_or("missing id")?;

        let response = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                &format!("/api/v1/errors/{error_id}"),
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["id"], error_id);
        assert_eq!(body["service"], "test-svc");
        assert_eq!(body["group"]["total_count"], 1);
        assert!(body["incident_ids"].as_array().is_some());

        let response = router
            .oneshot(read_request(READ_KEY, "/api/v1/errors/ERR-missing")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
        assert_eq!(body["detail"], "Error ERR-missing not found.");

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
                error_request("sk_live_unknown_secret", valid_error_body())?,
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
            ),
            (
                error_request(READ_KEY, valid_error_body())?,
                StatusCode::FORBIDDEN,
                "insufficient_scope",
            ),
            (
                error_request(REVOKED_KEY, valid_error_body())?,
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
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
            .oneshot(error_request(INGEST_KEY, "{}")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "validation_error");
        assert!(body["errors"].get("service").is_some());
        assert_eq!(error_count(&state)?, 0);

        let response = router
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
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
                    .header("authorization", format!("Bearer {INGEST_KEY}"))
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
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
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
            .oneshot(error_request(INGEST_KEY, "{")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_request");
        assert_eq!(error_count(&state)?, 0);

        let response = router
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
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
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
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
        test_ingest_state_with_sink(Arc::new(NoopIngestEffectSink))
    }

    fn test_ingest_state_with_sink(
        effect_sink: Arc<dyn IngestEffectSink>,
    ) -> Result<IngestState, Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;

        seed_api_key(&mut store, "KEY-admin", ADMIN_KEY, "admin", None)?;
        seed_api_key(&mut store, "KEY-ingest", INGEST_KEY, "ingest-only", None)?;
        seed_api_key(&mut store, "KEY-read", READ_KEY, "read-only", None)?;
        seed_api_key(
            &mut store,
            "KEY-revoked",
            REVOKED_KEY,
            "ingest-only",
            Some("2026-05-28T20:05:00Z"),
        )?;

        Ok(IngestState::new_with_effect_sink(
            store,
            IngestConfig::default(),
            effect_sink,
        ))
    }

    fn test_ingest_state_with_webhook_scheduler(
        scheduler: Arc<dyn WebhookScheduler>,
        active_webhook: bool,
    ) -> Result<IngestState, Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;

        seed_api_key(&mut store, "KEY-admin", ADMIN_KEY, "admin", None)?;
        seed_api_key(&mut store, "KEY-ingest", INGEST_KEY, "ingest-only", None)?;
        seed_api_key(&mut store, "KEY-read", READ_KEY, "read-only", None)?;
        store.insert_webhook_subscription(WebhookSubscriptionInsert {
            id: "WHK-test".to_owned(),
            url: "https://example.test/hook".to_owned(),
            events: vec!["error.new_class".to_owned()],
            secret: "test-webhook-secret".to_owned(),
            active: active_webhook,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;

        Ok(IngestState::new_with_webhook_scheduler(
            store,
            IngestConfig::default(),
            scheduler,
        ))
    }

    fn error_request(token: &str, body: &'static str) -> Result<Request<Body>, Box<dyn Error>> {
        Ok(Request::post("/api/v1/errors")
            .header("authorization", format!("Bearer {token}"))
            .header(CONTENT_TYPE, APPLICATION_JSON)
            .body(Body::from(body))?)
    }

    fn read_request(token: &str, path: &str) -> Result<Request<Body>, Box<dyn Error>> {
        Ok(Request::get(path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())?)
    }

    fn error_count(state: &IngestState) -> Result<u64, Box<dyn Error>> {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        Ok(store.error_count()?)
    }

    fn seed_api_key(
        store: &mut Store,
        id: &str,
        raw_key: &str,
        scope: &str,
        revoked_at: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        store.insert_api_key(ApiKeyInsert {
            id: id.to_owned(),
            name: format!("key {id}"),
            key_prefix: raw_key.chars().take(API_KEY_PREFIX_LEN).collect(),
            key_hash: bcrypt::hash(raw_key, bcrypt::DEFAULT_COST)?,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
            revoked_at: revoked_at.map(str::to_owned),
            scope: scope.to_owned(),
        })?;
        Ok(())
    }

    fn valid_error_body() -> &'static str {
        r#"{"service":"test-svc","error_class":"RuntimeError","message":"something went wrong"}"#
    }

    #[derive(Default)]
    struct RecordingFailingSink {
        effects: StdMutex<Vec<IngestEffect>>,
    }

    impl IngestEffectSink for RecordingFailingSink {
        fn handle(&self, effects: &[IngestEffect]) -> Result<(), String> {
            let mut recorded = self
                .effects
                .lock()
                .map_err(|_| "effect lock poisoned".to_owned())?;
            recorded.extend_from_slice(effects);
            Err("simulated effect sink failure".to_owned())
        }
    }

    #[derive(Default)]
    struct RecordingScheduler {
        jobs: StdMutex<Vec<WebhookJob>>,
    }

    impl WebhookScheduler for RecordingScheduler {
        fn schedule(&self, job: &WebhookJob) -> Result<(), String> {
            self.jobs
                .lock()
                .map_err(|_| "scheduler lock poisoned".to_owned())?
                .push(job.clone());
            Ok(())
        }
    }

    struct FailingScheduler;

    impl WebhookScheduler for FailingScheduler {
        fn schedule(&self, _job: &WebhookJob) -> Result<(), String> {
            Err("scheduler unavailable".to_owned())
        }
    }

    struct AlwaysCooldown;

    impl WebhookCooldown for AlwaysCooldown {
        fn in_cooldown(&self, _key: &str) -> bool {
            true
        }

        fn mark(&self, _key: &str) {}
    }
}
