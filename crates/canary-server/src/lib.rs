//! Axum server wiring for Canary.
//!
//! This crate adapts the stable wire contracts from `canary-http` to concrete
//! HTTP responses. Domain decisions and body shapes stay out of the router.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration as StdDuration,
};

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
use canary_store::{IncidentCorrelation, IncidentListOptions, Store};
use canary_store::{QueryError, ServiceQueryOptions};
use canary_workers::health::{
    HealthPlanError, MonitorCheckInInput, MonitorCheckInStatus, MonitorMode, MonitorSnapshot,
    ObservationContext, plan_monitor_check_in,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

mod webhooks;

use webhooks::NoopWebhookCooldown;
pub use webhooks::{
    HttpWebhookTransport, StoreWebhookScheduler, WebhookCircuit, WebhookCooldown,
    WebhookDeliveryDrain, WebhookDeliveryDrainReport, WebhookDeliveryDrainWorker,
    WebhookDeliveryRuntime, WebhookEnqueueEffectSink, WebhookScheduler, WebhookTransport,
};

const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const PROBLEM_CONTENT_TYPE: &str = "application/problem+json; charset=utf-8";
const DEFAULT_WEBHOOK_DRAIN_INTERVAL: StdDuration = StdDuration::from_secs(5);
const DEFAULT_WEBHOOK_DRAIN_MAX_JOBS: u32 = 25;

/// Runtime configuration for the top-level Canary server process.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// SQLite database path opened by the single-writer store.
    pub database_path: PathBuf,
    /// Ingest-domain limits and defaults.
    pub ingest: IngestConfig,
    /// Interval for the dedicated webhook delivery drain thread.
    pub webhook_drain_interval: StdDuration,
    /// Maximum scheduled webhook jobs claimed by one drain pass.
    pub webhook_drain_max_jobs: u32,
}

impl ServerConfig {
    /// Build a server configuration from an explicit SQLite database path.
    pub fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
            ingest: IngestConfig::default(),
            webhook_drain_interval: DEFAULT_WEBHOOK_DRAIN_INTERVAL,
            webhook_drain_max_jobs: DEFAULT_WEBHOOK_DRAIN_MAX_JOBS,
        }
    }
}

/// Fully wired Canary server runtime.
pub struct CanaryServer {
    router: Router,
    webhook_worker: WebhookDeliveryDrainWorker,
}

impl CanaryServer {
    /// Open storage, run migrations, wire HTTP routes, and start webhook draining.
    pub fn boot(config: ServerConfig) -> Result<Self, ServerBootError> {
        if config.webhook_drain_max_jobs == 0 {
            return Err(ServerBootError::InvalidConfig(
                "webhook drain max jobs must be greater than zero".to_owned(),
            ));
        }

        let mut store = Store::open(&config.database_path).map_err(ServerBootError::Store)?;
        store.migrate().map_err(ServerBootError::Store)?;
        let store = Arc::new(Mutex::new(store));

        let scheduler = Arc::new(StoreWebhookScheduler::new(store.clone()));
        let webhook_sink = Arc::new(WebhookEnqueueEffectSink::new(
            store.clone(),
            scheduler,
            Arc::new(NoopWebhookCooldown),
        ));
        let effect_sink = Arc::new(RuntimeIngestEffectSink::new(
            store.clone(),
            webhook_sink.clone(),
        ));
        let ingest_state = IngestState::new_with_shared_sinks(
            store.clone(),
            config.ingest,
            effect_sink,
            webhook_sink,
        );

        let transport = Arc::new(build_http_webhook_transport().map_err(ServerBootError::Http)?);
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport);
        let drain = WebhookDeliveryDrain::new(store, runtime, config.webhook_drain_max_jobs);
        let webhook_worker =
            WebhookDeliveryDrainWorker::spawn(drain, config.webhook_drain_interval)
                .map_err(ServerBootError::WebhookWorker)?;
        let router = public_router(PublicReadiness::ready()).merge(ingest_router(ingest_state));

        Ok(Self {
            router,
            webhook_worker,
        })
    }

    /// Return a clone of the composed public and authenticated router.
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// Serve the composed router until `shutdown` resolves, then stop the worker.
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServerRunError> {
        let router = self.router.clone();
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(ServerRunError::Listen)?;
        self.webhook_worker
            .join()
            .map_err(ServerRunError::WebhookWorker)
    }
}

/// Failure while booting the Canary server runtime.
#[derive(Debug)]
pub enum ServerBootError {
    /// Configuration is internally inconsistent.
    InvalidConfig(String),
    /// SQLite store open or migration failed.
    Store(canary_store::StoreError),
    /// HTTP webhook client initialization failed.
    Http(String),
    /// Webhook drain worker failed to start.
    WebhookWorker(String),
}

impl fmt::Display for ServerBootError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(error) => formatter.write_str(error),
            Self::Store(error) => write!(formatter, "store boot failed: {error}"),
            Self::Http(error) => formatter.write_str(error),
            Self::WebhookWorker(error) => formatter.write_str(error),
        }
    }
}

impl Error for ServerBootError {}

/// Failure while serving the Canary server runtime.
#[derive(Debug)]
pub enum ServerRunError {
    /// The Axum listener failed while serving requests.
    Listen(std::io::Error),
    /// The webhook worker did not shut down cleanly.
    WebhookWorker(String),
}

impl fmt::Display for ServerRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Listen(error) => write!(formatter, "server listen failed: {error}"),
            Self::WebhookWorker(error) => formatter.write_str(error),
        }
    }
}

impl Error for ServerRunError {}

fn build_http_webhook_transport() -> Result<HttpWebhookTransport, String> {
    thread::Builder::new()
        .name("canary-webhook-transport-init".to_owned())
        .spawn(HttpWebhookTransport::try_new)
        .map_err(|error| format!("failed to spawn webhook transport initializer: {error}"))?
        .join()
        .map_err(|_| "webhook transport initializer panicked".to_owned())?
}

/// Runtime sink for ingest post-commit effects.
pub struct RuntimeIngestEffectSink {
    store: Arc<Mutex<Store>>,
    webhook_sink: Arc<WebhookEnqueueEffectSink>,
}

impl RuntimeIngestEffectSink {
    /// Build the runtime effect sink from explicit persistence and webhook boundaries.
    pub fn new(store: Arc<Mutex<Store>>, webhook_sink: Arc<WebhookEnqueueEffectSink>) -> Self {
        Self {
            store,
            webhook_sink,
        }
    }
}

impl IngestEffectSink for RuntimeIngestEffectSink {
    fn handle(&self, effects: &[IngestEffect]) -> Result<(), String> {
        let mut errors = Vec::new();
        for effect in effects {
            let result = match effect {
                IngestEffect::BroadcastNewError { .. } => Ok(()),
                IngestEffect::CorrelateIncident {
                    signal_type,
                    signal_ref,
                    service,
                } => self.correlate_incident(signal_type, signal_ref, service),
                IngestEffect::EnqueueWebhook {
                    event,
                    payload_json,
                } => self.webhook_sink.enqueue_event(event, payload_json),
            };

            if let Err(error) = result {
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

impl RuntimeIngestEffectSink {
    fn correlate_incident(
        &self,
        signal_type: &str,
        signal_ref: &str,
        service: &str,
    ) -> Result<(), String> {
        let event = {
            let mut store = self
                .store
                .lock()
                .map_err(|_| "store lock poisoned".to_owned())?;
            store
                .correlate_incident(IncidentCorrelation {
                    signal_type: signal_type.to_owned(),
                    signal_ref: signal_ref.to_owned(),
                    service: service.to_owned(),
                    incident_id: canary_core::ids::IncidentId::generate(),
                    event_id: canary_core::ids::EventId::generate(),
                    now: current_rfc3339(),
                })
                .map_err(|error| error.to_string())?
        };

        if let Some(event) = event {
            self.webhook_sink
                .enqueue_event(&event.event, &event.payload_json)?;
        }

        Ok(())
    }
}

fn current_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// Sink for already-recorded service events that should fan out to webhooks.
pub trait EventSink: Send + Sync + 'static {
    /// Enqueue one event payload. Errors are advisory after the store commit.
    fn enqueue_event(&self, event: &str, payload_json: &str) -> Result<(), String>;
}

#[derive(Debug, Default)]
struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn enqueue_event(&self, _event: &str, _payload_json: &str) -> Result<(), String> {
        Ok(())
    }
}

impl EventSink for WebhookEnqueueEffectSink {
    fn enqueue_event(&self, event: &str, payload_json: &str) -> Result<(), String> {
        WebhookEnqueueEffectSink::enqueue_event(self, event, payload_json)
    }
}

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
        .route("/api/v1/check-ins", post(create_check_in))
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
    event_sink: Arc<dyn EventSink>,
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
        let webhook_sink = Arc::new(WebhookEnqueueEffectSink::new(
            store.clone(),
            scheduler,
            Arc::new(NoopWebhookCooldown),
        ));
        Self {
            store,
            config,
            effect_sink: webhook_sink.clone(),
            event_sink: webhook_sink,
        }
    }

    /// Build ingest state with an explicit post-commit effect sink.
    pub fn new_with_effect_sink(
        store: Store,
        config: IngestConfig,
        effect_sink: Arc<dyn IngestEffectSink>,
    ) -> Self {
        Self::new_with_shared_effect_sink(Arc::new(Mutex::new(store)), config, effect_sink)
    }

    /// Build ingest state from a shared single-writer store and explicit effect sink.
    pub fn new_with_shared_effect_sink(
        store: Arc<Mutex<Store>>,
        config: IngestConfig,
        effect_sink: Arc<dyn IngestEffectSink>,
    ) -> Self {
        Self {
            store,
            config,
            effect_sink,
            event_sink: Arc::new(NoopEventSink),
        }
    }

    /// Build ingest state from shared store plus explicit ingest and event sinks.
    pub fn new_with_shared_sinks(
        store: Arc<Mutex<Store>>,
        config: IngestConfig,
        effect_sink: Arc<dyn IngestEffectSink>,
        event_sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            store,
            config,
            effect_sink,
            event_sink,
        }
    }
}

/// Best-effort sink for ingest effects emitted after the store transaction commits.
pub trait IngestEffectSink: Send + Sync + 'static {
    /// Handle effects. Errors are advisory and must not change the HTTP response.
    fn handle(&self, effects: &[IngestEffect]) -> Result<(), String>;
}

#[derive(Debug, Default)]
struct NoopIngestEffectSink;

impl IngestEffectSink for NoopIngestEffectSink {
    fn handle(&self, _effects: &[IngestEffect]) -> Result<(), String> {
        Ok(())
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

async fn create_check_in(
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

    let mut store = match state.store.lock() {
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
        let _ = state
            .event_sink
            .enqueue_event(&transition.event, &transition.payload_json);
        if let Some(event) = transition.incident_event {
            let _ = state
                .event_sink
                .enqueue_event(&event.event, &event.payload_json);
        }
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

struct ParsedCheckIn {
    monitor_name: String,
    observed_at: String,
    input: MonitorCheckInInput,
}

fn parse_check_in(attrs: Map<String, Value>) -> Result<ParsedCheckIn, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = BTreeMap::new();
    let monitor_name = required_string(&attrs, "monitor", &mut errors);
    let status = parse_check_in_status(attrs.get("status"), &mut errors);

    if !errors.is_empty() {
        return Err(Box::new(check_in_validation_problem(errors)));
    }

    let Some(monitor_name) = monitor_name else {
        return Err(Box::new(check_in_validation_problem(BTreeMap::new())));
    };
    let Some(status) = status else {
        return Err(Box::new(check_in_validation_problem(BTreeMap::new())));
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

fn required_string(
    attrs: &Map<String, Value>,
    key: &str,
    errors: &mut ValidationErrors,
) -> Option<String> {
    match attrs.get(key) {
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => {
            errors.insert(
                key.to_owned(),
                vec!["must be a non-empty string".to_owned()],
            );
            None
        }
    }
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

fn optional_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
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

fn monitor_snapshot(
    snapshot: canary_store::MonitorCheckInSnapshot,
) -> Result<MonitorSnapshot, String> {
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
    match value {
        "unknown" => Ok(canary_core::health::state_machine::HealthState::Unknown),
        "up" => Ok(canary_core::health::state_machine::HealthState::Up),
        "degraded" => Ok(canary_core::health::state_machine::HealthState::Degraded),
        "down" => Ok(canary_core::health::state_machine::HealthState::Down),
        "paused" => Ok(canary_core::health::state_machine::HealthState::Paused),
        "flapping" => Ok(canary_core::health::state_machine::HealthState::Flapping),
        _ => Err(format!("unknown health state: {value}")),
    }
}

fn current_unix_millis() -> i64 {
    let nanos = time::OffsetDateTime::now_utc().unix_timestamp_nanos();
    i64::try_from(nanos / 1_000_000).unwrap_or(i64::MAX)
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

fn check_in_validation_problem(errors: ValidationErrors) -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid check-in payload.",
        None,
    )
    .with_extra("errors", json!(errors))
}

fn invalid_observed_at_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid observed_at timestamp.",
        None,
    )
    .with_extra(
        "errors",
        json!({"observed_at": ["must be an ISO8601 timestamp"]}),
    )
}

fn not_found_problem(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(404, ProblemCode::NotFound, detail, None)
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
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::process;
    use std::sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };
    use std::thread::{self, JoinHandle, ThreadId};
    use std::time::{Duration as StdDuration, Instant};

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header::CONTENT_TYPE},
    };
    use canary_http::public::{APPLICATION_JSON, OPENAPI_JSON};
    use canary_store::{
        API_KEY_PREFIX_LEN, ApiKeyInsert, MonitorInsert, WebhookDeliveryJobInsert,
        WebhookDeliveryJobState, WebhookDeliveryStatus, WebhookSubscriptionInsert,
    };
    use canary_workers::webhooks::{CircuitDecision, TransportResult, WebhookJob, WebhookRequest};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    const ADMIN_KEY: &str = "sk_live_admin_secret";
    const INGEST_KEY: &str = "sk_live_ingest_secret";
    const READ_KEY: &str = "sk_live_read_secret";
    const REVOKED_KEY: &str = "sk_live_revoked_secret";
    static TEMP_DB_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    async fn canary_server_boots_public_and_authenticated_routes() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("routes");
        let config = ServerConfig {
            webhook_drain_interval: StdDuration::from_secs(60),
            ..ServerConfig::new(path.clone())
        };
        let server = CanaryServer::boot(config)?;

        let health = server
            .router()
            .oneshot(Request::get("/healthz").body(Body::empty())?)
            .await?;
        assert_eq!(health.status(), StatusCode::OK);

        let query = server
            .router()
            .oneshot(read_request(READ_KEY, "/api/v1/query?service=test-svc")?)
            .await?;
        assert_eq!(
            query.status(),
            StatusCode::UNAUTHORIZED,
            "boot should not seed implicit API keys"
        );

        drop_server(server).await?;
        fs::remove_file(path)?;

        Ok(())
    }

    #[tokio::test]
    async fn canary_server_boot_wires_ingest_to_webhook_delivery() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("webhooks");
        let (url, http_server) = spawn_webhook_server(204, &[])?;
        {
            let mut store = Store::open(&path)?;
            store.migrate()?;
            seed_api_key(&mut store, "KEY-ingest", INGEST_KEY, "ingest-only", None)?;
            store.insert_webhook_subscription(WebhookSubscriptionInsert {
                id: "WHK-boot".to_owned(),
                url,
                events: vec!["error.new_class".to_owned()],
                secret: "test-webhook-secret".to_owned(),
                active: true,
                created_at: "2026-05-28T20:00:00Z".to_owned(),
            })?;
        }

        let config = ServerConfig {
            webhook_drain_interval: StdDuration::from_millis(10),
            ..ServerConfig::new(path.clone())
        };
        let server = CanaryServer::boot(config)?;
        let response = server
            .router()
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);
        wait_for_delivered_webhook(&path)?;
        let captured = join_http_server(http_server)?;
        assert!(captured.head.starts_with("POST /hook HTTP/1.1"));
        assert_eq!(
            header_value(&captured.head, "x-event").as_deref(),
            Some("error.new_class")
        );
        assert!(captured.body.contains(r#""service":"test-svc""#));

        drop_server(server).await?;
        fs::remove_file(path)?;

        Ok(())
    }

    #[tokio::test]
    async fn canary_server_boot_wires_ingest_to_incident_correlation() -> Result<(), Box<dyn Error>>
    {
        let path = temp_db_path("incidents");
        {
            let mut store = Store::open(&path)?;
            store.migrate()?;
            seed_api_key(&mut store, "KEY-ingest", INGEST_KEY, "ingest-only", None)?;
            seed_api_key(&mut store, "KEY-read", READ_KEY, "read-only", None)?;
        }

        let config = ServerConfig {
            webhook_drain_interval: StdDuration::from_secs(60),
            ..ServerConfig::new(path.clone())
        };
        let server = CanaryServer::boot(config)?;
        let response = server
            .router()
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        assert_eq!(response.status(), StatusCode::CREATED);

        let incidents = server
            .router()
            .oneshot(read_request(READ_KEY, "/api/v1/incidents")?)
            .await?;
        assert_eq!(incidents.status(), StatusCode::OK);
        let body = json_body(incidents).await?;
        assert_eq!(body["incidents"][0]["service"], "test-svc");
        assert_eq!(body["incidents"][0]["signal_count"], 1);
        assert_eq!(
            body["incidents"][0]["signals"][0]["signal_type"],
            "error_group"
        );

        drop_server(server).await?;
        fs::remove_file(path)?;

        Ok(())
    }

    #[tokio::test]
    async fn canary_server_boot_enqueues_incident_webhook_events() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("incident-webhooks");
        let (url, http_server) = spawn_webhook_server(204, &[])?;
        {
            let mut store = Store::open(&path)?;
            store.migrate()?;
            seed_api_key(&mut store, "KEY-ingest", INGEST_KEY, "ingest-only", None)?;
            store.insert_webhook_subscription(WebhookSubscriptionInsert {
                id: "WHK-incident".to_owned(),
                url,
                events: vec!["incident.opened".to_owned()],
                secret: "test-webhook-secret".to_owned(),
                active: true,
                created_at: "2026-05-28T20:00:00Z".to_owned(),
            })?;
        }

        let config = ServerConfig {
            webhook_drain_interval: StdDuration::from_millis(10),
            ..ServerConfig::new(path.clone())
        };
        let server = CanaryServer::boot(config)?;
        let response = server
            .router()
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        assert_eq!(response.status(), StatusCode::CREATED);

        wait_for_delivered_webhook(&path)?;
        let captured = join_http_server(http_server)?;
        assert_eq!(
            header_value(&captured.head, "x-event").as_deref(),
            Some("incident.opened")
        );
        assert!(captured.body.contains(r#""event":"incident.opened""#));

        drop_server(server).await?;
        fs::remove_file(path)?;

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

    #[test]
    fn webhook_delivery_runtime_delivers_and_records_success() -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let transport = Arc::new(RecordingTransport::status(204));
        let circuit = Arc::new(RecordingCircuit::closed());
        let runtime =
            WebhookDeliveryRuntime::new(store.clone(), transport.clone(), circuit.clone());
        let execution = runtime.deliver(&webhook_job("DLV-runtime-ok", 1, 4))?;

        assert_eq!(
            execution.outcome,
            canary_workers::webhooks::DeliveryOutcome::Delivered
        );
        let requests = transport
            .requests
            .lock()
            .map_err(|_| "transport lock poisoned")?;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].headers.delivery_id, "DLV-runtime-ok");
        drop(requests);

        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some("DLV-runtime-ok".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Delivered);
        assert_eq!(rows[0].attempt_count, 1);
        assert!(rows[0].delivered_at.is_some());
        assert_eq!(
            circuit
                .successes
                .lock()
                .map_err(|_| "circuit lock poisoned")?
                .as_slice(),
            ["WHK-test"]
        );

        Ok(())
    }

    #[test]
    fn webhook_delivery_runtime_retries_failed_attempt_without_final_discard()
    -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let transport = Arc::new(RecordingTransport::status(500));
        let circuit = Arc::new(RecordingCircuit::closed());
        let runtime = WebhookDeliveryRuntime::new(store.clone(), transport, circuit.clone());
        let execution = runtime.deliver(&webhook_job("DLV-runtime-retry", 2, 4))?;

        assert_eq!(execution.retry_after_seconds, Some(5));
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some("DLV-runtime-retry".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Retrying);
        assert_eq!(rows[0].attempt_count, 1);
        assert_eq!(rows[0].discarded_at, None);
        assert_eq!(
            circuit
                .failures
                .lock()
                .map_err(|_| "circuit lock poisoned")?
                .as_slice(),
            ["WHK-test"]
        );

        Ok(())
    }

    #[test]
    fn webhook_delivery_runtime_suppresses_open_circuit_without_transport()
    -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let transport = Arc::new(RecordingTransport::status(204));
        let circuit = Arc::new(RecordingCircuit::open());
        let runtime = WebhookDeliveryRuntime::new(store.clone(), transport.clone(), circuit);
        let execution = runtime.deliver(&webhook_job("DLV-runtime-open", 1, 4))?;

        assert_eq!(
            execution.outcome,
            canary_workers::webhooks::DeliveryOutcome::Suppressed {
                reason: "circuit_open".to_owned()
            }
        );
        assert_eq!(
            transport
                .requests
                .lock()
                .map_err(|_| "transport lock poisoned")?
                .len(),
            0
        );

        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some("DLV-runtime-open".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Suppressed);
        assert_eq!(rows[0].reason.as_deref(), Some("circuit_open"));
        assert_eq!(rows[0].attempt_count, 0);

        Ok(())
    }

    #[test]
    fn webhook_delivery_runtime_discards_missing_and_inactive_without_transport()
    -> Result<(), Box<dyn Error>> {
        let store = runtime_store(false)?;
        let transport = Arc::new(RecordingTransport::status(204));
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport.clone());

        runtime.deliver(&webhook_job("DLV-runtime-inactive", 1, 4))?;
        runtime.deliver(&WebhookJob {
            webhook_id: "WHK-missing".to_owned(),
            delivery_id: Some("DLV-runtime-missing".to_owned()),
            ..webhook_job("DLV-unused", 1, 4)
        })?;

        assert_eq!(
            transport
                .requests
                .lock()
                .map_err(|_| "transport lock poisoned")?
                .len(),
            0
        );
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let inactive = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some("DLV-runtime-inactive".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(inactive[0].status, WebhookDeliveryStatus::Discarded);
        assert_eq!(inactive[0].reason.as_deref(), Some("webhook_inactive"));

        let missing = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some("DLV-runtime-missing".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(missing[0].status, WebhookDeliveryStatus::Discarded);
        assert_eq!(missing[0].reason.as_deref(), Some("webhook_not_found"));

        Ok(())
    }

    #[test]
    fn http_webhook_transport_sends_signed_body_and_maps_status() -> Result<(), Box<dyn Error>> {
        let (url, server) = spawn_webhook_server(202, &[])?;
        let request = WebhookRequest {
            url,
            body: r#"{"event":"error.new_class","ok":true}"#.to_owned(),
            headers: canary_http::webhooks::headers_for_body(
                r#"{"event":"error.new_class","ok":true}"#,
                "test-webhook-secret",
                "error.new_class",
                "DLV-http-ok",
                Some(42),
            ),
        };
        let transport = HttpWebhookTransport::try_new()?;

        let result = transport.send(&request);
        let captured = join_http_server(server)?;

        assert_eq!(result, TransportResult::HttpStatus(202));
        assert!(captured.head.starts_with("POST /hook HTTP/1.1"));
        assert_eq!(captured.body, request.body);
        assert!(
            canary_http::webhooks::verify_signature(
                captured.body.as_bytes(),
                "test-webhook-secret",
                &request.headers.signature,
            ),
            "receiver should be able to verify signature over exact received bytes"
        );
        assert_eq!(
            header_value(&captured.head, "content-type").as_deref(),
            Some("application/json")
        );
        assert_eq!(
            header_value(&captured.head, "x-event").as_deref(),
            Some("error.new_class")
        );
        assert_eq!(
            header_value(&captured.head, "x-delivery-id").as_deref(),
            Some("DLV-http-ok")
        );
        assert_eq!(
            header_value(&captured.head, "x-webhook-version").as_deref(),
            Some("1")
        );
        assert_eq!(
            header_value(&captured.head, "x-sequence").as_deref(),
            Some("42")
        );
        assert_eq!(
            header_value(&captured.head, "x-signature").as_deref(),
            Some(request.headers.signature.as_str())
        );

        Ok(())
    }

    #[test]
    fn http_webhook_transport_does_not_follow_redirects_or_retry() -> Result<(), Box<dyn Error>> {
        let (url, server) =
            spawn_webhook_server(307, &[("location", "http://127.0.0.1:1/second")])?;
        let request = WebhookRequest {
            url,
            body: "{}".to_owned(),
            headers: canary_http::webhooks::headers_for_body(
                "{}",
                "test-webhook-secret",
                "error.new_class",
                "DLV-http-redirect",
                None,
            ),
        };
        let transport = HttpWebhookTransport::try_new()?;

        let result = transport.send(&request);
        let captured = join_http_server(server)?;

        assert_eq!(result, TransportResult::HttpStatus(307));
        assert!(captured.head.starts_with("POST /hook HTTP/1.1"));
        assert_eq!(captured.body, "{}");

        Ok(())
    }

    #[test]
    fn http_webhook_transport_leaves_failure_status_for_scheduler() -> Result<(), Box<dyn Error>> {
        let (url, server) = spawn_webhook_server(503, &[])?;
        let request = WebhookRequest {
            url,
            body: "{}".to_owned(),
            headers: canary_http::webhooks::headers_for_body(
                "{}",
                "test-webhook-secret",
                "error.new_class",
                "DLV-http-503",
                None,
            ),
        };
        let transport = HttpWebhookTransport::try_new()?;

        let result = transport.send(&request);
        let captured = join_http_server(server)?;

        assert_eq!(result, TransportResult::HttpStatus(503));
        assert_eq!(captured.body, "{}");

        Ok(())
    }

    #[test]
    fn http_webhook_transport_maps_connection_failures_to_request_errors()
    -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        drop(listener);
        let request = WebhookRequest {
            url: format!("http://{addr}/hook"),
            body: "{}".to_owned(),
            headers: canary_http::webhooks::headers_for_body(
                "{}",
                "test-webhook-secret",
                "error.new_class",
                "DLV-http-error",
                None,
            ),
        };
        let transport = HttpWebhookTransport::with_timeout(StdDuration::from_millis(200))?;

        let TransportResult::RequestError(reason) = transport.send(&request) else {
            return Err("connection failure should map to request error".into());
        };
        assert!(!reason.is_empty());

        Ok(())
    }

    #[test]
    fn webhook_delivery_runtime_uses_http_transport_and_records_ledger()
    -> Result<(), Box<dyn Error>> {
        let (url, server) = spawn_webhook_server(204, &[])?;
        let store = runtime_store_with_url(true, &url)?;
        let runtime = WebhookDeliveryRuntime::new_without_circuit(
            store.clone(),
            Arc::new(HttpWebhookTransport::try_new()?),
        );

        let execution = runtime.deliver(&webhook_job("DLV-runtime-http", 1, 4))?;
        let captured = join_http_server(server)?;

        assert_eq!(
            execution.outcome,
            canary_workers::webhooks::DeliveryOutcome::Delivered
        );
        assert_eq!(
            captured.body,
            r#"{"error":{"group_hash":"group-runtime"},"sequence":7}"#
        );
        assert_eq!(
            header_value(&captured.head, "x-delivery-id").as_deref(),
            Some("DLV-runtime-http")
        );
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some("DLV-runtime-http".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Delivered);
        assert_eq!(rows[0].attempt_count, 1);

        Ok(())
    }

    #[test]
    fn store_webhook_scheduler_persists_claimable_job_args() -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let scheduler = StoreWebhookScheduler::new(store.clone());

        scheduler.schedule(&webhook_job("DLV-scheduled", 1, 4))?;

        let mut store = store.lock().map_err(|_| "store lock poisoned")?;
        let jobs = store.claim_due_webhook_delivery_jobs("9999-01-01T00:00:00Z", 10)?;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].args["delivery_id"], "DLV-scheduled");
        assert_eq!(jobs[0].args["webhook_id"], "WHK-test");
        assert_eq!(jobs[0].attempt, 1);
        assert_eq!(jobs[0].max_attempts, 4);

        Ok(())
    }

    #[test]
    fn webhook_delivery_drain_delivers_due_job_and_marks_completed() -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let scheduler = StoreWebhookScheduler::new(store.clone());
        scheduler.schedule(&webhook_job("DLV-drain-ok", 1, 4))?;
        let transport = Arc::new(RecordingTransport::status(204));
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport.clone());
        let drain = WebhookDeliveryDrain::new(store.clone(), runtime, 10);

        let report = drain.drain_due("9999-01-01T00:00:00Z")?;

        assert_eq!(
            report,
            WebhookDeliveryDrainReport {
                claimed: 1,
                completed: 1,
                retried: 0,
                discarded: 0,
            }
        );
        assert_eq!(
            transport
                .requests
                .lock()
                .map_err(|_| "transport lock poisoned")?
                .len(),
            1
        );
        let mut store = store.lock().map_err(|_| "store lock poisoned")?;
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some("DLV-drain-ok".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Delivered);
        assert!(
            store
                .claim_due_webhook_delivery_jobs("9999-01-01T00:00:00Z", 10)?
                .is_empty()
        );

        Ok(())
    }

    #[test]
    fn webhook_delivery_drain_reschedules_retry_with_same_delivery_id() -> Result<(), Box<dyn Error>>
    {
        let store = runtime_store(true)?;
        let job_id = insert_due_webhook_job(&store, "DLV-drain-retry", 4)?;
        let transport = Arc::new(RecordingTransport::status(500));
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport);
        let drain = WebhookDeliveryDrain::new(store.clone(), runtime, 10);

        let report = drain.drain_due("2026-05-28T20:00:00Z")?;

        assert_eq!(
            report,
            WebhookDeliveryDrainReport {
                claimed: 1,
                completed: 0,
                retried: 1,
                discarded: 0,
            }
        );
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let job = store
            .webhook_delivery_job(job_id)?
            .ok_or("missing webhook delivery job")?;
        assert_eq!(job.state, WebhookDeliveryJobState::Scheduled);
        assert_eq!(job.scheduled_at, "2026-05-28T20:00:01Z");
        assert_eq!(job.attempt, 1);
        assert_eq!(job.args["delivery_id"], "DLV-drain-retry");
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some("DLV-drain-retry".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Retrying);
        assert_eq!(rows[0].attempt_count, 1);
        assert_eq!(rows[0].discarded_at, None);

        Ok(())
    }

    #[test]
    fn webhook_delivery_drain_discards_final_failure() -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let job_id = insert_due_webhook_job(&store, "DLV-drain-final", 2)?;
        let transport = Arc::new(RecordingTransport::status(500));
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport);
        let drain = WebhookDeliveryDrain::new(store.clone(), runtime, 10);

        let first = drain.drain_due("2026-05-28T20:00:00Z")?;
        let second = drain.drain_due("2026-05-28T20:00:01Z")?;

        assert_eq!(first.retried, 1);
        assert_eq!(
            second,
            WebhookDeliveryDrainReport {
                claimed: 1,
                completed: 0,
                retried: 0,
                discarded: 1,
            }
        );
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        assert_eq!(
            store
                .webhook_delivery_job(job_id)?
                .ok_or("missing webhook delivery job")?
                .state,
            WebhookDeliveryJobState::Discarded
        );
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some("DLV-drain-final".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Discarded);
        assert_eq!(rows[0].reason.as_deref(), Some("http_500"));
        assert_eq!(rows[0].attempt_count, 2);

        Ok(())
    }

    #[test]
    fn webhook_delivery_drain_open_circuit_completes_without_transport_or_retry()
    -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let job_id = insert_due_webhook_job(&store, "DLV-drain-open", 4)?;
        let transport = Arc::new(RecordingTransport::status(204));
        let runtime = WebhookDeliveryRuntime::new(
            store.clone(),
            transport.clone(),
            Arc::new(RecordingCircuit::open()),
        );
        let drain = WebhookDeliveryDrain::new(store.clone(), runtime, 10);

        let report = drain.drain_due("2026-05-28T20:00:00Z")?;

        assert_eq!(
            report,
            WebhookDeliveryDrainReport {
                claimed: 1,
                completed: 1,
                retried: 0,
                discarded: 0,
            }
        );
        assert_eq!(
            transport
                .requests
                .lock()
                .map_err(|_| "transport lock poisoned")?
                .len(),
            0
        );
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        assert_eq!(
            store
                .webhook_delivery_job(job_id)?
                .ok_or("missing webhook delivery job")?
                .state,
            WebhookDeliveryJobState::Completed
        );
        let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            delivery_id: Some("DLV-drain-open".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Suppressed);
        assert_eq!(rows[0].reason.as_deref(), Some("circuit_open"));
        assert_eq!(rows[0].attempt_count, 0);

        Ok(())
    }

    #[test]
    fn webhook_delivery_drain_worker_runs_delivery_on_dedicated_thread()
    -> Result<(), Box<dyn Error>> {
        let test_thread_id = thread::current().id();
        let store = runtime_store(true)?;
        let scheduler = StoreWebhookScheduler::new(store.clone());
        scheduler.schedule(&webhook_job("DLV-worker-ok", 1, 4))?;
        let transport = Arc::new(ThreadRecordingTransport::status(204));
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport.clone());
        let drain = WebhookDeliveryDrain::new(store.clone(), runtime, 10);

        let worker = WebhookDeliveryDrainWorker::spawn(drain, StdDuration::from_secs(60))?;

        wait_for_delivery_status(&store, "DLV-worker-ok", WebhookDeliveryStatus::Delivered)?;
        worker.join()?;
        let thread_ids = transport.thread_ids()?;

        assert_eq!(thread_ids.len(), 1);
        assert_ne!(thread_ids[0], test_thread_id);
        let mut store = store.lock().map_err(|_| "store lock poisoned")?;
        assert!(
            store
                .claim_due_webhook_delivery_jobs("9999-01-01T00:00:00Z", 10)?
                .is_empty()
        );

        Ok(())
    }

    #[test]
    fn webhook_delivery_drain_worker_stop_wakes_sleeping_thread() -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let runtime = WebhookDeliveryRuntime::new_without_circuit(
            store.clone(),
            Arc::new(RecordingTransport::status(204)),
        );
        let drain = WebhookDeliveryDrain::new(store, runtime, 10);
        let worker = WebhookDeliveryDrainWorker::spawn(drain, StdDuration::from_secs(60))?;
        let started = Instant::now();

        worker.join()?;

        assert!(started.elapsed() < StdDuration::from_secs(2));
        Ok(())
    }

    #[test]
    fn webhook_delivery_drain_worker_rejects_zero_interval() -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let runtime = WebhookDeliveryRuntime::new_without_circuit(
            store.clone(),
            Arc::new(RecordingTransport::status(204)),
        );
        let drain = WebhookDeliveryDrain::new(store, runtime, 10);

        let error = WebhookDeliveryDrainWorker::spawn(drain, StdDuration::ZERO)
            .err()
            .ok_or("zero interval should be rejected")?;

        assert_eq!(error, "webhook drain interval must be greater than zero");
        Ok(())
    }

    #[test]
    fn webhook_delivery_drain_worker_survives_panicking_transport() -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let scheduler = StoreWebhookScheduler::new(store.clone());
        scheduler.schedule(&webhook_job("DLV-worker-panic", 1, 4))?;
        scheduler.schedule(&webhook_job("DLV-worker-after-panic", 1, 4))?;
        let transport = Arc::new(PanicOnceTransport::new());
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport);
        let drain = WebhookDeliveryDrain::new(store.clone(), runtime, 1);

        let worker = WebhookDeliveryDrainWorker::spawn(drain, StdDuration::from_millis(10))?;

        wait_for_delivery_status(
            &store,
            "DLV-worker-after-panic",
            WebhookDeliveryStatus::Delivered,
        )?;
        worker.join()?;

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
    async fn monitor_check_in_accepts_ingest_scope_and_returns_phoenix_body()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state_with_monitor("desktop-active-timer")?;
        let response = ingest_router(state)
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"desktop-active-timer","status":"alive","observed_at":"2026-05-28T20:00:00Z","ttl_ms":120000}"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["monitor_id"], "MON-desktop-active-timer");
        assert_eq!(body["state"], "up");
        assert_eq!(body["observed_at"], "2026-05-28T20:00:00Z");
        assert_eq!(body["sequence"], 1);

        Ok(())
    }

    #[tokio::test]
    async fn monitor_check_in_enqueues_transition_webhook() -> Result<(), Box<dyn Error>> {
        let scheduler = Arc::new(RecordingScheduler::default());
        let state = test_ingest_state_with_monitor_webhook(
            "desktop-active-timer",
            scheduler.clone(),
            "health_check.recovered",
        )?;

        let response = ingest_router(state.clone())
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"desktop-active-timer","status":"alive","observed_at":"2026-05-28T20:00:00Z"}"#,
            )?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);
        let jobs = scheduler
            .jobs
            .lock()
            .map_err(|_| "scheduler lock poisoned")?;
        assert_eq!(jobs.len(), 1);
        let job = jobs.first().ok_or("missing scheduled webhook job")?;
        assert_eq!(job.webhook_id, "WHK-monitor");
        assert_eq!(job.event, "health_check.recovered");
        assert_eq!(job.payload["monitor"]["name"], "desktop-active-timer");
        assert_eq!(job.payload["state"], "up");

        Ok(())
    }

    #[tokio::test]
    async fn monitor_check_in_returns_404_for_unknown_monitor() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let response = ingest_router(state)
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"missing","status":"alive"}"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");

        Ok(())
    }

    #[tokio::test]
    async fn monitor_check_in_reports_payload_validation_errors() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state_with_monitor("desktop-active-timer")?;
        let response = ingest_router(state)
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"desktop-active-timer"}"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "validation_error");
        assert_eq!(
            body["errors"]["status"],
            json!(["must be one of: alive, in_progress, ok, error"])
        );

        Ok(())
    }

    #[tokio::test]
    async fn monitor_check_in_reports_invalid_observed_at() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state_with_monitor("desktop-active-timer")?;
        let response = ingest_router(state)
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"desktop-active-timer","status":"alive","observed_at":"not-a-time"}"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["detail"], "Invalid observed_at timestamp.");
        assert_eq!(
            body["errors"]["observed_at"],
            json!(["must be an ISO8601 timestamp"])
        );

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

    fn test_ingest_state_with_monitor(name: &str) -> Result<IngestState, Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
            seed_monitor(&mut store, name)?;
        }
        Ok(state)
    }

    fn test_ingest_state_with_monitor_webhook(
        name: &str,
        scheduler: Arc<dyn WebhookScheduler>,
        event: &str,
    ) -> Result<IngestState, Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;

        seed_api_key(&mut store, "KEY-admin", ADMIN_KEY, "admin", None)?;
        seed_api_key(&mut store, "KEY-ingest", INGEST_KEY, "ingest-only", None)?;
        seed_api_key(&mut store, "KEY-read", READ_KEY, "read-only", None)?;
        seed_monitor(&mut store, name)?;
        store.insert_webhook_subscription(WebhookSubscriptionInsert {
            id: "WHK-monitor".to_owned(),
            url: "https://example.test/monitor".to_owned(),
            events: vec![event.to_owned()],
            secret: "test-webhook-secret".to_owned(),
            active: true,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;

        Ok(IngestState::new_with_webhook_scheduler(
            store,
            IngestConfig::default(),
            scheduler,
        ))
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

    fn temp_db_path(name: &str) -> PathBuf {
        let id = TEMP_DB_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("canary-server-{name}-{}-{id}.db", process::id()))
    }

    fn wait_for_delivered_webhook(path: &Path) -> Result<(), Box<dyn Error>> {
        let deadline = Instant::now() + StdDuration::from_secs(3);
        loop {
            let store = Store::open(path)?;
            let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
                status: Some(WebhookDeliveryStatus::Delivered),
                limit: Some(1),
                ..Default::default()
            })?;
            if !rows.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for delivered webhook".into());
            }
            thread::sleep(StdDuration::from_millis(20));
        }
    }

    async fn drop_server(server: CanaryServer) -> Result<(), Box<dyn Error>> {
        tokio::task::spawn_blocking(move || drop(server)).await?;
        Ok(())
    }

    fn error_request(token: &str, body: &'static str) -> Result<Request<Body>, Box<dyn Error>> {
        Ok(Request::post("/api/v1/errors")
            .header("authorization", format!("Bearer {token}"))
            .header(CONTENT_TYPE, APPLICATION_JSON)
            .body(Body::from(body))?)
    }

    fn check_in_request(token: &str, body: &'static str) -> Result<Request<Body>, Box<dyn Error>> {
        Ok(Request::post("/api/v1/check-ins")
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

    fn seed_monitor(store: &mut Store, name: &str) -> Result<(), Box<dyn Error>> {
        store.insert_monitor(MonitorInsert {
            id: format!("MON-{name}"),
            name: name.to_owned(),
            service: name.to_owned(),
            mode: "ttl".to_owned(),
            expected_every_ms: 90_000,
            grace_ms: 5_000,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        Ok(())
    }

    fn valid_error_body() -> &'static str {
        r#"{"service":"test-svc","error_class":"RuntimeError","message":"something went wrong"}"#
    }

    fn runtime_store(active_webhook: bool) -> Result<Arc<Mutex<Store>>, Box<dyn Error>> {
        runtime_store_with_url(active_webhook, "https://example.test/hook")
    }

    fn runtime_store_with_url(
        active_webhook: bool,
        url: &str,
    ) -> Result<Arc<Mutex<Store>>, Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        store.insert_webhook_subscription(WebhookSubscriptionInsert {
            id: "WHK-test".to_owned(),
            url: url.to_owned(),
            events: vec!["error.new_class".to_owned()],
            secret: "test-webhook-secret".to_owned(),
            active: active_webhook,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;

        Ok(Arc::new(Mutex::new(store)))
    }

    fn webhook_job(delivery_id: &str, attempt: u32, max_attempts: u32) -> WebhookJob {
        WebhookJob {
            webhook_id: "WHK-test".to_owned(),
            payload: json!({
                "error": {"group_hash": "group-runtime"},
                "sequence": 7
            }),
            event: "error.new_class".to_owned(),
            delivery_id: Some(delivery_id.to_owned()),
            legacy_job_id: None,
            attempt,
            max_attempts,
        }
    }

    #[derive(Debug)]
    struct CapturedHttpRequest {
        head: String,
        body: String,
    }

    type HttpServerHandle = JoinHandle<std::io::Result<CapturedHttpRequest>>;

    fn spawn_webhook_server(
        status: u16,
        headers: &[(&str, &str)],
    ) -> Result<(String, HttpServerHandle), Box<dyn Error>> {
        // One accepted connection is intentional: redirect following or hidden
        // retries should show up as the original status, not extra requests.
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let headers = headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<Vec<_>>();
        let handle = thread::spawn(move || -> std::io::Result<CapturedHttpRequest> {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(StdDuration::from_secs(2)))?;
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !http_request_complete(&bytes) {
                let read = stream.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..read]);
            }

            let mut response = format!("HTTP/1.1 {status} test\r\ncontent-length: 0\r\n");
            for (name, value) in headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("connection: close\r\n\r\n");
            stream.write_all(response.as_bytes())?;

            let raw = String::from_utf8(bytes)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let Some((head, body)) = raw.split_once("\r\n\r\n") else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "request missing header terminator",
                ));
            };
            Ok(CapturedHttpRequest {
                head: head.to_owned(),
                body: body.to_owned(),
            })
        });

        Ok((format!("http://{addr}/hook"), handle))
    }

    fn join_http_server(handle: HttpServerHandle) -> std::io::Result<CapturedHttpRequest> {
        handle
            .join()
            .map_err(|_| std::io::Error::other("HTTP test server panicked"))?
    }

    fn http_request_complete(bytes: &[u8]) -> bool {
        let raw = String::from_utf8_lossy(bytes);
        let Some((head, body)) = raw.split_once("\r\n\r\n") else {
            return false;
        };
        let content_length = header_value(head, "content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);

        body.len() >= content_length
    }

    fn header_value(head: &str, header: &str) -> Option<String> {
        head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case(header) {
                Some(value.trim().to_owned())
            } else {
                None
            }
        })
    }

    fn insert_due_webhook_job(
        store: &Arc<Mutex<Store>>,
        delivery_id: &str,
        max_attempts: u32,
    ) -> Result<i64, Box<dyn Error>> {
        let mut store = store.lock().map_err(|_| "store lock poisoned")?;
        Ok(store.insert_webhook_delivery_job(WebhookDeliveryJobInsert {
            args: json!({
                "webhook_id": "WHK-test",
                "payload": {
                    "error": {"group_hash": "group-runtime"},
                    "sequence": 7
                },
                "event": "error.new_class",
                "delivery_id": delivery_id
            }),
            scheduled_at: "2026-05-28T20:00:00Z".to_owned(),
            now: "2026-05-28T20:00:00Z".to_owned(),
            max_attempts,
        })?)
    }

    fn wait_for_delivery_status(
        store: &Arc<Mutex<Store>>,
        delivery_id: &str,
        status: WebhookDeliveryStatus,
    ) -> Result<(), Box<dyn Error>> {
        let deadline = Instant::now() + StdDuration::from_secs(2);
        loop {
            {
                let store = store.lock().map_err(|_| "store lock poisoned")?;
                let rows = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
                    delivery_id: Some(delivery_id.to_owned()),
                    ..Default::default()
                })?;
                if rows.first().is_some_and(|row| row.status == status) {
                    return Ok(());
                }
            }

            if Instant::now() >= deadline {
                return Err(
                    format!("timed out waiting for {delivery_id} to become {status:?}").into(),
                );
            }
            thread::sleep(StdDuration::from_millis(10));
        }
    }

    struct RecordingTransport {
        response: TransportResult,
        requests: StdMutex<Vec<WebhookRequest>>,
    }

    impl RecordingTransport {
        fn status(status: u16) -> Self {
            Self {
                response: TransportResult::HttpStatus(status),
                requests: StdMutex::new(Vec::new()),
            }
        }
    }

    impl WebhookTransport for RecordingTransport {
        fn send(&self, request: &WebhookRequest) -> TransportResult {
            if let Ok(mut requests) = self.requests.lock() {
                requests.push(request.clone());
            }
            self.response.clone()
        }
    }

    struct ThreadRecordingTransport {
        response: TransportResult,
        thread_ids: StdMutex<Vec<ThreadId>>,
    }

    impl ThreadRecordingTransport {
        fn status(status: u16) -> Self {
            Self {
                response: TransportResult::HttpStatus(status),
                thread_ids: StdMutex::new(Vec::new()),
            }
        }

        fn thread_ids(&self) -> Result<Vec<ThreadId>, Box<dyn Error>> {
            self.thread_ids
                .lock()
                .map(|thread_ids| thread_ids.clone())
                .map_err(|_| "thread id lock poisoned".into())
        }
    }

    impl WebhookTransport for ThreadRecordingTransport {
        fn send(&self, _request: &WebhookRequest) -> TransportResult {
            if let Ok(mut thread_ids) = self.thread_ids.lock() {
                thread_ids.push(thread::current().id());
            }
            self.response.clone()
        }
    }

    struct PanicOnceTransport {
        should_panic: AtomicBool,
    }

    impl PanicOnceTransport {
        fn new() -> Self {
            Self {
                should_panic: AtomicBool::new(true),
            }
        }
    }

    impl WebhookTransport for PanicOnceTransport {
        fn send(&self, _request: &WebhookRequest) -> TransportResult {
            if self.should_panic.swap(false, Ordering::SeqCst) {
                std::panic::resume_unwind(Box::new("test transport panic"));
            }
            TransportResult::HttpStatus(204)
        }
    }

    struct RecordingCircuit {
        decision: CircuitDecision,
        successes: StdMutex<Vec<String>>,
        failures: StdMutex<Vec<String>>,
    }

    impl RecordingCircuit {
        fn closed() -> Self {
            Self {
                decision: CircuitDecision::Closed,
                successes: StdMutex::new(Vec::new()),
                failures: StdMutex::new(Vec::new()),
            }
        }

        fn open() -> Self {
            Self {
                decision: CircuitDecision::Open,
                successes: StdMutex::new(Vec::new()),
                failures: StdMutex::new(Vec::new()),
            }
        }
    }

    impl WebhookCircuit for RecordingCircuit {
        fn decision(&self, _webhook_id: &str) -> CircuitDecision {
            self.decision
        }

        fn record_success(&self, webhook_id: &str) {
            if let Ok(mut successes) = self.successes.lock() {
                successes.push(webhook_id.to_owned());
            }
        }

        fn record_failure(&self, webhook_id: &str) {
            if let Ok(mut failures) = self.failures.lock() {
                failures.push(webhook_id.to_owned());
            }
        }
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
