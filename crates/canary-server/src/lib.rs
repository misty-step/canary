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
    extract::{Path, Query, RawQuery, State},
    http::{
        HeaderMap, HeaderValue, Response, StatusCode,
        header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderName},
    },
    routing::{delete, get, patch, post},
};
use canary_core::query::{
    ErrorGroupSummary, ReportCursor, decode_report_cursor, encode_report_cursor,
};
use canary_http::public::{
    DependencyStatus, PublicResponse, healthz_response, openapi_response, readyz_response,
};
use canary_http::{
    auth::{
        ApiKeyScope, BearerToken, Permission, extract_bearer, insufficient_scope_problem,
        invalid_api_key_problem, missing_authorization_problem,
    },
    problem_details::{ProblemCode, ProblemDetails},
    rate_limit::{RateLimitKind, rate_limited_problem},
    request::{
        MAX_JSON_BODY_BYTES, decode_json_object,
        payload_too_large_problem as http_payload_too_large_problem,
    },
};
use canary_ingest::{
    IngestConfig, IngestContext, IngestEffect, IngestError, ValidationErrors,
    ingest as ingest_error,
};
use canary_store::{
    AnnotationError, AnnotationInsert, AnnotationPageOptions, ApiKeyInsert, ApiKeyRecord,
    ErrorSummaryItem, HealthMonitorStatus, HealthTargetStatus, IncidentCorrelation,
    IncidentListOptions, MonitorInsert, MonitorRecord, RecentTransition, SearchResult, Store,
    StoreError, TargetCheckRead, TargetConflict, TargetInsert, TargetRecord, VerifiedApiKey,
    WebhookSubscription, WebhookSubscriptionInsert,
};
use canary_store::{QueryError, ServiceQueryOptions, TimelineQueryError, TimelineQueryOptions};
use canary_store::{WebhookDeliveryPageError, WebhookDeliveryPageOptions};
use canary_workers::health::{
    HealthPlanError, MonitorCheckInInput, MonitorCheckInStatus, MonitorMode, MonitorSnapshot,
    ObservationContext, plan_monitor_check_in,
};
use canary_workers::webhooks::{
    TransportResult, WebhookEndpoint, WebhookJob, WebhookRequest, build_request,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

mod health_fanout;
mod monitor_overdue;
mod rate_limit;
mod target_probes;
mod webhooks;

pub use health_fanout::{
    EnqueueFailure, EnqueueFailureKey, EnqueueFailureRecorder, EnqueueFailureSink,
    EventFanoutReport, HealthEventFanout, HealthEventSource,
};
pub use monitor_overdue::{
    MonitorOverdueLifecycle, MonitorOverdueLifecycleConfig, MonitorOverdueLifecycleReport,
    MonitorOverdueLifecycleWorker, MonitorOverdueOutcome, MonitorOverdueRuntime,
    MonitorOverdueRuntimeError, run_monitor_overdue_once,
};
use rate_limit::{RateLimitDecision, RateLimiter};
pub use target_probes::{
    ProbeHttpResponse, ProbeRequest, ProbeTransport, ProbeTransportError, ReqwestProbeTransport,
    TargetProbeLifecycle, TargetProbeLifecycleCommand, TargetProbeLifecycleConfig,
    TargetProbeLifecycleController, TargetProbeLifecycleReport, TargetProbeLifecycleWorker,
    TargetProbeOptions, TargetProbeOutcome, TargetProbeRuntime, TargetProbeRuntimeError,
    run_target_probe_once, validate_target_configuration,
};
use webhooks::NoopWebhookCooldown;
pub use webhooks::{
    HttpWebhookTransport, StoreWebhookScheduler, WebhookCircuit, WebhookCooldown,
    WebhookDeliveryDrain, WebhookDeliveryDrainReport, WebhookDeliveryDrainWorker,
    WebhookDeliveryRuntime, WebhookEnqueueEffectSink, WebhookScheduler, WebhookTransport,
};

const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const PROBLEM_CONTENT_TYPE: &str = "application/problem+json; charset=utf-8";
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

fn default_webhook_transport() -> Arc<dyn WebhookTransport> {
    Arc::new(LazyHttpWebhookTransport)
}

struct LazyHttpWebhookTransport;

impl WebhookTransport for LazyHttpWebhookTransport {
    fn send(&self, request: &WebhookRequest) -> TransportResult {
        let request = request.clone();
        match HttpWebhookTransport::try_new() {
            Ok(transport) => transport.send(&request),
            Err(error) => TransportResult::RequestError(error),
        }
    }
}
const DEFAULT_WEBHOOK_DRAIN_INTERVAL: StdDuration = StdDuration::from_secs(5);
const DEFAULT_WEBHOOK_DRAIN_MAX_JOBS: u32 = 25;
const DEFAULT_TARGET_PROBE_INTERVAL: StdDuration = StdDuration::from_secs(1);
const DEFAULT_MONITOR_OVERDUE_INTERVAL: StdDuration = StdDuration::from_secs(1);

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
    /// Minimum interval between active target probe lifecycle passes.
    pub target_probe_interval: StdDuration,
    /// Runtime options for HTTP target probes.
    pub target_probe_options: TargetProbeOptions,
    /// Minimum interval between non-HTTP monitor overdue evaluation passes.
    pub monitor_overdue_interval: StdDuration,
}

impl ServerConfig {
    /// Build a server configuration from an explicit SQLite database path.
    pub fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
            ingest: IngestConfig::default(),
            webhook_drain_interval: DEFAULT_WEBHOOK_DRAIN_INTERVAL,
            webhook_drain_max_jobs: DEFAULT_WEBHOOK_DRAIN_MAX_JOBS,
            target_probe_interval: DEFAULT_TARGET_PROBE_INTERVAL,
            target_probe_options: TargetProbeOptions::default(),
            monitor_overdue_interval: DEFAULT_MONITOR_OVERDUE_INTERVAL,
        }
    }
}

/// Fully wired Canary server runtime.
pub struct CanaryServer {
    router: Router,
    webhook_worker: WebhookDeliveryDrainWorker,
    target_probe_worker: TargetProbeLifecycleWorker,
    monitor_overdue_worker: MonitorOverdueLifecycleWorker,
    enqueue_failure_sink: Arc<EnqueueFailureRecorder>,
}

impl CanaryServer {
    /// Open storage, run migrations, wire HTTP routes, and start webhook draining.
    pub fn boot(config: ServerConfig) -> Result<Self, ServerBootError> {
        if config.webhook_drain_max_jobs == 0 {
            return Err(ServerBootError::InvalidConfig(
                "webhook drain max jobs must be greater than zero".to_owned(),
            ));
        }
        if config.target_probe_interval.is_zero() {
            return Err(ServerBootError::InvalidConfig(
                "target probe interval must be greater than zero".to_owned(),
            ));
        }
        if config.monitor_overdue_interval.is_zero() {
            return Err(ServerBootError::InvalidConfig(
                "monitor overdue interval must be greater than zero".to_owned(),
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
        let enqueue_failure_sink = Arc::new(EnqueueFailureRecorder::default());
        let health_fanout =
            HealthEventFanout::new(webhook_sink.clone(), enqueue_failure_sink.clone());
        let ingest_state = IngestState::new_with_shared_fanout(
            store.clone(),
            config.ingest,
            effect_sink,
            health_fanout.clone(),
        );

        let transport = Arc::new(build_http_webhook_transport().map_err(ServerBootError::Http)?);
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport);
        let drain = WebhookDeliveryDrain::new(store, runtime, config.webhook_drain_max_jobs);
        let webhook_worker =
            WebhookDeliveryDrainWorker::spawn(drain, config.webhook_drain_interval)
                .map_err(ServerBootError::WebhookWorker)?;
        let allow_private_targets = config.target_probe_options.allow_private_targets;
        let target_transport: Arc<dyn ProbeTransport> = Arc::new(ReqwestProbeTransport);
        let target_runtime = TargetProbeRuntime::new(
            ingest_state.store.clone(),
            health_fanout.clone(),
            target_transport,
            config.target_probe_options,
        );
        let target_probe_worker = TargetProbeLifecycleWorker::spawn(
            TargetProbeLifecycle::new(ingest_state.store.clone(), target_runtime),
            TargetProbeLifecycleConfig {
                tick_interval: config.target_probe_interval,
            },
        )
        .map_err(ServerBootError::TargetProbeWorker)?;
        let monitor_overdue_worker = MonitorOverdueLifecycleWorker::spawn(
            MonitorOverdueLifecycle::new(
                ingest_state.store.clone(),
                MonitorOverdueRuntime::new(ingest_state.store.clone(), health_fanout),
            ),
            MonitorOverdueLifecycleConfig {
                tick_interval: config.monitor_overdue_interval,
            },
        )
        .map_err(ServerBootError::MonitorOverdueWorker)?;
        let ingest_state = ingest_state
            .with_target_control(Arc::new(target_probe_worker.controller()))
            .with_allow_private_targets(allow_private_targets);
        let router = public_router(PublicReadiness::ready()).merge(ingest_router(ingest_state));

        Ok(Self {
            router,
            webhook_worker,
            target_probe_worker,
            monitor_overdue_worker,
            enqueue_failure_sink,
        })
    }

    /// Return a clone of the composed public and authenticated router.
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// Return health-transition webhook enqueue failures observed by this process.
    pub fn enqueue_failure_snapshot(&self) -> BTreeMap<EnqueueFailureKey, u64> {
        self.enqueue_failure_sink.snapshot()
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
        self.target_probe_worker
            .join()
            .map_err(ServerRunError::TargetProbeWorker)?;
        self.monitor_overdue_worker
            .join()
            .map_err(ServerRunError::MonitorOverdueWorker)?;
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
    /// Target probe lifecycle worker failed to start.
    TargetProbeWorker(String),
    /// Monitor overdue lifecycle worker failed to start.
    MonitorOverdueWorker(String),
}

impl fmt::Display for ServerBootError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(error) => formatter.write_str(error),
            Self::Store(error) => write!(formatter, "store boot failed: {error}"),
            Self::Http(error) => formatter.write_str(error),
            Self::WebhookWorker(error) => formatter.write_str(error),
            Self::TargetProbeWorker(error) => formatter.write_str(error),
            Self::MonitorOverdueWorker(error) => formatter.write_str(error),
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
    /// The target probe worker did not shut down cleanly.
    TargetProbeWorker(String),
    /// The monitor overdue worker did not shut down cleanly.
    MonitorOverdueWorker(String),
}

impl fmt::Display for ServerRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Listen(error) => write!(formatter, "server listen failed: {error}"),
            Self::WebhookWorker(error) => formatter.write_str(error),
            Self::TargetProbeWorker(error) => formatter.write_str(error),
            Self::MonitorOverdueWorker(error) => formatter.write_str(error),
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
        .route("/metrics", get(metrics))
        .route("/api/v1/errors", post(create_error))
        .route("/api/v1/check-ins", post(create_check_in))
        .route("/api/v1/query", get(query_errors))
        .route("/api/v1/report", get(report))
        .route("/api/v1/timeline", get(timeline))
        .route("/api/v1/webhook-deliveries", get(webhook_deliveries))
        .route("/api/v1/status", get(status))
        .route("/api/v1/health-status", get(health_status))
        .route("/api/v1/targets/{id}/checks", get(target_checks))
        .route("/api/v1/incidents", get(list_incidents))
        .route("/api/v1/incidents/{id}", get(show_incident))
        .route(
            "/api/v1/incidents/{incident_id}/annotations",
            get(list_incident_annotations).post(create_incident_annotation),
        )
        .route(
            "/api/v1/groups/{group_hash}/annotations",
            get(list_group_annotations).post(create_group_annotation),
        )
        .route(
            "/api/v1/annotations",
            get(list_annotations).post(create_annotation),
        )
        .route("/api/v1/errors/{id}", get(show_error))
        .route("/api/v1/monitors", get(list_monitors).post(create_monitor))
        .route("/api/v1/monitors/{id}", delete(delete_monitor))
        .route("/api/v1/webhooks", get(list_webhooks).post(create_webhook))
        .route("/api/v1/webhooks/{id}", delete(delete_webhook))
        .route("/api/v1/webhooks/{id}/test", post(test_webhook))
        .route("/api/v1/keys", get(list_api_keys).post(create_api_key))
        .route("/api/v1/keys/{id}/revoke", post(revoke_api_key))
        .route(
            "/api/v1/service-onboarding",
            post(create_service_onboarding),
        )
        .route("/api/v1/targets", get(list_targets).post(create_target))
        .route(
            "/api/v1/targets/{id}",
            patch(update_target_interval).delete(delete_target),
        )
        .route("/api/v1/targets/{id}/pause", post(pause_target))
        .route("/api/v1/targets/{id}/resume", post(resume_target))
        .with_state(state)
}

/// Shared state needed by authenticated ingest routes.
#[derive(Clone)]
pub struct IngestState {
    store: Arc<Mutex<Store>>,
    config: IngestConfig,
    effect_sink: Arc<dyn IngestEffectSink>,
    health_fanout: HealthEventFanout,
    target_control: Arc<dyn TargetControlSink>,
    webhook_transport: Arc<dyn WebhookTransport>,
    rate_limiter: Arc<Mutex<RateLimiter>>,
    allow_private_targets: bool,
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
            health_fanout: HealthEventFanout::new_without_failure_sink(webhook_sink),
            target_control: Arc::new(NoopTargetControlSink),
            webhook_transport: default_webhook_transport(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::default())),
            allow_private_targets: false,
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
            health_fanout: HealthEventFanout::new_without_failure_sink(Arc::new(NoopEventSink)),
            target_control: Arc::new(NoopTargetControlSink),
            webhook_transport: default_webhook_transport(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::default())),
            allow_private_targets: false,
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
            health_fanout: HealthEventFanout::new_without_failure_sink(event_sink),
            target_control: Arc::new(NoopTargetControlSink),
            webhook_transport: default_webhook_transport(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::default())),
            allow_private_targets: false,
        }
    }

    /// Build ingest state from shared store plus explicit ingest and health fanout sinks.
    pub fn new_with_shared_fanout(
        store: Arc<Mutex<Store>>,
        config: IngestConfig,
        effect_sink: Arc<dyn IngestEffectSink>,
        health_fanout: HealthEventFanout,
    ) -> Self {
        Self {
            store,
            config,
            effect_sink,
            health_fanout,
            target_control: Arc::new(NoopTargetControlSink),
            webhook_transport: default_webhook_transport(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::default())),
            allow_private_targets: false,
        }
    }

    /// Attach the target probe lifecycle control boundary used by admin routes.
    pub fn with_target_control(mut self, target_control: Arc<dyn TargetControlSink>) -> Self {
        self.target_control = target_control;
        self
    }

    /// Attach the outbound webhook transport used by the admin test route.
    pub fn with_webhook_transport(mut self, webhook_transport: Arc<dyn WebhookTransport>) -> Self {
        self.webhook_transport = webhook_transport;
        self
    }

    /// Allow admin target creation to accept private/non-global probe hosts.
    pub fn with_allow_private_targets(mut self, allow_private_targets: bool) -> Self {
        self.allow_private_targets = allow_private_targets;
        self
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

/// Narrow control boundary from admin target writes to the probe lifecycle.
pub trait TargetControlSink: Send + Sync + 'static {
    /// Apply one target-scoped lifecycle command.
    fn control_target(&self, command: TargetProbeLifecycleCommand) -> Result<(), String>;
}

#[derive(Debug, Default)]
struct NoopTargetControlSink;

impl TargetControlSink for NoopTargetControlSink {
    fn control_target(&self, _command: TargetProbeLifecycleCommand) -> Result<(), String> {
        Ok(())
    }
}

impl TargetControlSink for TargetProbeLifecycleController {
    fn control_target(&self, command: TargetProbeLifecycleCommand) -> Result<(), String> {
        TargetProbeLifecycleController::control_target(self, command)
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
        let _fanout_report = state
            .health_fanout
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

async fn list_targets(State(state): State<IngestState>, headers: HeaderMap) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
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

async fn list_monitors(State(state): State<IngestState>, headers: HeaderMap) -> Response<Body> {
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

async fn create_monitor(
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
        Ok(false) => problem_response(monitor_validation_problem(BTreeMap::from([(
            "name".to_owned(),
            vec!["has already been taken".to_owned()],
        )]))),
        Err(_) => problem_response(internal_problem()),
    }
}

async fn delete_monitor(
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

async fn list_webhooks(State(state): State<IngestState>, headers: HeaderMap) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.webhook_subscriptions() {
        Ok(webhooks) => json_status_response(
            StatusCode::OK.as_u16(),
            json!({"webhooks": webhooks.into_iter().map(webhook_response).collect::<Vec<_>>()}),
        ),
        Err(_) => problem_response(internal_problem()),
    }
}

async fn metrics(State(state): State<IngestState>, headers: HeaderMap) -> Response<Body> {
    if let Err(problem) = require_query_limited_admin_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.metrics_snapshot() {
        Ok(snapshot) => response(
            StatusCode::OK.as_u16(),
            PROMETHEUS_CONTENT_TYPE,
            Body::from(canary_core::metrics::render_prometheus(&snapshot)),
        ),
        Err(_) => problem_response(internal_problem()),
    }
}

async fn create_webhook(
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
    let webhook = match parse_webhook_create(attrs) {
        Ok(webhook) => webhook,
        Err(problem) => return problem_response(*problem),
    };
    let response_body = webhook_insert_response(&webhook);

    let mut store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.insert_webhook_subscription(webhook) {
        Ok(()) => json_status_response(StatusCode::CREATED.as_u16(), response_body),
        Err(_) => problem_response(internal_problem()),
    }
}

async fn delete_webhook(
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
    match store.delete_webhook_subscription(&id) {
        Ok(true) => response(
            StatusCode::NO_CONTENT.as_u16(),
            "text/plain; charset=utf-8",
            Body::empty(),
        ),
        Ok(false) => problem_response(not_found_problem("Webhook not found.")),
        Err(_) => problem_response(internal_problem()),
    }
}

async fn list_api_keys(State(state): State<IngestState>, headers: HeaderMap) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.list_api_keys() {
        Ok(keys) => json_status_response(
            StatusCode::OK.as_u16(),
            json!({"keys": keys.into_iter().map(api_key_response).collect::<Vec<_>>()}),
        ),
        Err(_) => problem_response(internal_problem()),
    }
}

async fn create_api_key(
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
    };
    let response_body = api_key_insert_response(&key, &raw_key);

    let mut store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    match store.insert_api_key(key) {
        Ok(()) => json_status_response(StatusCode::CREATED.as_u16(), response_body),
        Err(_) => problem_response(internal_problem()),
    }
}

async fn create_service_onboarding(
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
    let request = match parse_service_onboarding_create(attrs, state.allow_private_targets) {
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

    let mut store = match state.store.lock() {
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

    let _control_result = state.target_control.control_target(command);

    json_status_response(StatusCode::CREATED.as_u16(), response_body)
}

async fn revoke_api_key(
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
    match store.revoke_api_key(&id, &current_rfc3339()) {
        Ok(true) => json_status_response(StatusCode::OK.as_u16(), json!({"status": "revoked"})),
        Ok(false) => problem_response(not_found_problem("API key not found.")),
        Err(_) => problem_response(internal_problem()),
    }
}

async fn test_webhook(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }

    let subscription = {
        let store = match state.store.lock() {
            Ok(store) => store,
            Err(_) => return problem_response(internal_problem()),
        };
        match store.webhook_subscription(&id) {
            Ok(Some(subscription)) => subscription,
            Ok(None) => return problem_response(not_found_problem("Webhook not found.")),
            Err(_) => return problem_response(internal_problem()),
        }
    };

    let endpoint = webhook_endpoint(subscription);
    let payload = json!({
        "event": "canary.ping",
        "message": "Webhook test from Canary",
        "test": true,
        "timestamp": current_rfc3339(),
    });
    let job = WebhookJob {
        webhook_id: endpoint.id.clone(),
        payload,
        event: "canary.ping".to_owned(),
        delivery_id: Some(canary_core::ids::DeliveryId::generate().into_string()),
        legacy_job_id: None,
        attempt: 1,
        max_attempts: 1,
    };
    let Some(request) = build_request(&endpoint, &job) else {
        return problem_response(webhook_delivery_failed_problem("webhook_inactive"));
    };

    let transport = state.webhook_transport.clone();
    match tokio::task::spawn_blocking(move || transport.send(&request)).await {
        Ok(result) => match result {
            TransportResult::HttpStatus(status) if (200..=299).contains(&status) => {
                json_status_response(StatusCode::OK.as_u16(), json!({"status": "delivered"}))
            }
            TransportResult::HttpStatus(status) => {
                problem_response(webhook_delivery_failed_problem(format!("HTTP {status}")))
            }
            TransportResult::RequestError(reason) => {
                problem_response(webhook_delivery_failed_problem(reason))
            }
        },
        Err(error) => problem_response(webhook_delivery_failed_problem(error.to_string())),
    }
}

async fn create_target(
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
    let target = match parse_target_create(attrs, state.allow_private_targets) {
        Ok(target) => target,
        Err(problem) => return problem_response(*problem),
    };
    let command = TargetProbeLifecycleCommand::Track {
        target_id: target.id.clone(),
        interval_ms: target.interval_ms,
    };
    let response_body = target_insert_response(&target);

    let mut store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    if store.insert_target(target).is_err() {
        return problem_response(internal_problem());
    }
    drop(store);

    let _control_result = state.target_control.control_target(command);

    json_status_response(StatusCode::CREATED.as_u16(), response_body)
}

async fn delete_target(
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
    match store.delete_target(&id) {
        Ok(true) => {}
        Ok(false) => return problem_response(not_found_problem("Target not found.")),
        Err(_) => return problem_response(internal_problem()),
    }
    drop(store);

    let _control_result = state
        .target_control
        .control_target(TargetProbeLifecycleCommand::Untrack { target_id: id });

    response(
        StatusCode::NO_CONTENT.as_u16(),
        "text/plain; charset=utf-8",
        Body::empty(),
    )
}

async fn update_target_interval(
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

    let mut store = match state.store.lock() {
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
        let _control_result =
            state
                .target_control
                .control_target(TargetProbeLifecycleCommand::Reconfigure {
                    target_id: id,
                    interval_ms: update.target.interval_ms,
                });
    }

    json_status_response(StatusCode::OK.as_u16(), target_response(update.target))
}

async fn pause_target(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    set_target_active(state, headers, id, false).await
}

async fn resume_target(
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

    let mut store = match state.store.lock() {
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
    let _control_result = state.target_control.control_target(command);

    json_status_response(
        StatusCode::OK.as_u16(),
        json!({"status": if active { "resumed" } else { "paused" }}),
    )
}

async fn query_errors(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<QueryParams>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
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

async fn timeline(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<TimelineParams>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let window = params.window.as_deref().unwrap_or("24h");
    let cursor = params.after.or(params.cursor);
    let options = TimelineQueryOptions {
        service: params.service,
        limit: params.limit,
        cursor,
        event_type: params.event_type,
    };
    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.timeline(window, options) {
        Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
        Err(TimelineQueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(TimelineQueryError::InvalidLimit) => problem_response(invalid_limit_problem()),
        Err(TimelineQueryError::InvalidCursor) => problem_response(invalid_cursor_problem()),
        Err(TimelineQueryError::InvalidEventType(invalid)) => {
            problem_response(invalid_event_type_problem(&invalid))
        }
        Err(TimelineQueryError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

async fn webhook_deliveries(
    State(state): State<IngestState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Query(params): Query<WebhookDeliveryParams>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    if query_param_is_array(raw_query.as_deref(), "webhook_id") {
        return problem_response(invalid_string_param_problem("webhook_id"));
    }
    if query_param_is_array(raw_query.as_deref(), "event") {
        return problem_response(invalid_string_param_problem("event"));
    }
    if query_param_is_array(raw_query.as_deref(), "status") {
        return problem_response(invalid_string_param_problem("status"));
    }

    let cursor = params.after.or(params.cursor);
    let options = WebhookDeliveryPageOptions {
        webhook_id: params.webhook_id,
        event: params.event,
        status: params.status,
        limit: params.limit,
        cursor,
    };
    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.webhook_delivery_page(options) {
        Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
        Err(WebhookDeliveryPageError::InvalidLimit) => problem_response(invalid_limit_problem()),
        Err(WebhookDeliveryPageError::InvalidCursor) => problem_response(invalid_cursor_problem()),
        Err(WebhookDeliveryPageError::InvalidStatus) => {
            problem_response(invalid_webhook_delivery_status_problem())
        }
        Err(WebhookDeliveryPageError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

async fn health_status(State(state): State<IngestState>, headers: HeaderMap) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    let targets = match store.health_targets() {
        Ok(targets) => targets,
        Err(_) => return problem_response(internal_problem()),
    };
    let monitors = match store.health_monitors() {
        Ok(monitors) => monitors,
        Err(_) => return problem_response(internal_problem()),
    };

    json_status_response(
        StatusCode::OK.as_u16(),
        json!({
            "summary": health_status_summary(&targets, &monitors),
            "targets": targets.iter().map(health_target_response).collect::<Vec<_>>(),
            "monitors": monitors.iter().map(health_monitor_response).collect::<Vec<_>>(),
        }),
    )
}

async fn status(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<StatusParams>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let window = params.window.as_deref().unwrap_or("1h");
    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    let targets = match store.health_targets() {
        Ok(targets) => targets,
        Err(_) => return problem_response(internal_problem()),
    };
    let monitors = match store.health_monitors() {
        Ok(monitors) => monitors,
        Err(_) => return problem_response(internal_problem()),
    };
    let error_summary = match store.error_summary(window) {
        Ok(summary) => summary,
        Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
    };
    let overall = combined_overall(&targets, &monitors, &error_summary);

    json_status_response(
        StatusCode::OK.as_u16(),
        json!({
            "overall": overall,
            "summary": combined_status_summary(&overall, &targets, &monitors, &error_summary, window),
            "targets": targets.iter().map(status_target_response).collect::<Vec<_>>(),
            "monitors": monitors.iter().map(status_monitor_response).collect::<Vec<_>>(),
            "error_summary": error_summary.iter().map(error_summary_response).collect::<Vec<_>>(),
        }),
    )
}

async fn report(
    State(state): State<IngestState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Query(params): Query<ReportParams>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }
    if query_param_is_array(raw_query.as_deref(), "q") {
        return problem_response(invalid_string_param_problem("q"));
    }

    let window = params.window.as_deref().unwrap_or("1h");
    let limit = match parse_report_limit(params.limit.as_deref()) {
        Ok(limit) => limit,
        Err(problem) => return problem_response(*problem),
    };
    let cursor = match parse_report_cursor(params.cursor.as_deref()) {
        Ok(cursor) => cursor,
        Err(problem) => return problem_response(*problem),
    };

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    let targets = match store.health_targets() {
        Ok(targets) => targets,
        Err(_) => return problem_response(internal_problem()),
    };
    let monitors = match store.health_monitors() {
        Ok(monitors) => monitors,
        Err(_) => return problem_response(internal_problem()),
    };
    let error_summary = match store.error_summary(window) {
        Ok(summary) => summary,
        Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
    };
    let error_groups = match store.report_error_groups(window) {
        Ok(groups) => groups,
        Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
    };
    let incidents = match store.active_incidents(IncidentListOptions::default()) {
        Ok(incidents) => incidents.incidents,
        Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
    };
    let transitions = match store.recent_transitions(window) {
        Ok(transitions) => transitions,
        Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
    };
    let search_results = match params.q.as_deref() {
        Some(query) => match store.search_errors(query, window) {
            Ok(results) => Some(results),
            Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
            Err(QueryError::Sqlite(_)) => Some(Vec::new()),
        },
        None => None,
    };

    let overall = combined_overall(&targets, &monitors, &error_summary);
    let summary = combined_status_summary(&overall, &targets, &monitors, &error_summary, window);
    let (targets, next_targets_offset) =
        paginate_report_items(targets, limit, cursor.targets_offset);
    let (monitors, next_monitor_offset) =
        paginate_report_items(monitors, limit, cursor.monitor_offset);
    let (error_groups, next_error_groups_offset) = paginate_report_items(
        error_groups,
        Some(limit.unwrap_or(25)),
        cursor.error_groups_offset,
    );
    let next_cursor = ReportCursor {
        targets_offset: next_targets_offset,
        monitor_offset: next_monitor_offset,
        error_groups_offset: next_error_groups_offset,
    };
    let cursor = encode_report_cursor(&next_cursor);
    let truncated = cursor.is_some();

    let mut body = json!({
        "status": overall,
        "summary": summary,
        "targets": targets.iter().map(health_target_response).collect::<Vec<_>>(),
        "monitors": monitors.iter().map(health_monitor_response).collect::<Vec<_>>(),
        "error_groups": error_groups.iter().map(error_group_response).collect::<Vec<_>>(),
        "incidents": incidents,
        "recent_transitions": transitions.iter().map(recent_transition_response).collect::<Vec<_>>(),
        "truncated": truncated,
        "cursor": cursor,
    });
    if let (Some(results), Some(object)) = (search_results, body.as_object_mut()) {
        object.insert(
            "search_results".to_owned(),
            Value::Array(results.iter().map(search_result_response).collect()),
        );
    }

    if accepts_csv(&headers) {
        response(
            StatusCode::OK.as_u16(),
            "text/csv; charset=utf-8",
            Body::from(report_csv(&body)),
        )
    } else {
        json_status_response(StatusCode::OK.as_u16(), body)
    }
}

async fn target_checks(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<StatusParams>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let window = params.window.as_deref().unwrap_or("24h");
    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    let checks = match store.target_checks(&id, window) {
        Ok(checks) => checks,
        Err(QueryError::InvalidWindow) => return problem_response(target_checks_window_problem()),
        Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
    };

    json_status_response(
        StatusCode::OK.as_u16(),
        json!({
            "target_id": id,
            "window": window,
            "checks": checks.iter().map(target_check_response).collect::<Vec<_>>(),
        }),
    )
}

async fn list_incident_annotations(
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

async fn create_incident_annotation(
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

async fn list_group_annotations(
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

async fn create_group_annotation(
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

async fn list_annotations(
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

async fn create_annotation(
    State(state): State<IngestState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if let Err(problem) = require_scope(&state, &headers, Permission::Admin) {
        return problem_response(*problem);
    }
    if let Err(problem) = check_content_length(&headers) {
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
    if let Err(problem) = check_content_length(&headers) {
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

#[derive(Deserialize)]
struct StatusParams {
    window: Option<String>,
}

#[derive(Deserialize)]
struct ReportParams {
    window: Option<String>,
    q: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct TimelineParams {
    service: Option<String>,
    window: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
    after: Option<String>,
    event_type: Option<String>,
}

#[derive(Deserialize)]
struct WebhookDeliveryParams {
    webhook_id: Option<String>,
    event: Option<String>,
    status: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
    after: Option<String>,
}

#[derive(Deserialize)]
struct AnnotationPageParams {
    subject_type: Option<String>,
    subject_id: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
}

struct ParsedCheckIn {
    monitor_name: String,
    observed_at: String,
    input: MonitorCheckInInput,
}

struct ApiKeyCreate {
    name: String,
    scope: String,
}

struct ServiceOnboardingCreate {
    service: String,
    url: String,
    environment: String,
    interval_ms: Option<i64>,
}

struct AnnotationCreate {
    subject_type: String,
    subject_id: String,
    agent: String,
    action: String,
    metadata: Option<Value>,
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

fn health_target_response(target: &HealthTargetStatus) -> Value {
    json!({
        "id": target.id,
        "name": target.name,
        "service": target.service,
        "url": target.url,
        "state": target.state,
        "consecutive_failures": target.consecutive_failures,
        "last_checked_at": target.last_checked_at,
        "last_success_at": target.last_success_at,
        "latency_ms": target.latency_ms,
        "tls_expires_at": target.tls_expires_at,
        "recent_checks": target.recent_checks.iter().map(|check| json!({
            "checked_at": check.checked_at,
            "result": check.result,
            "status_code": check.status_code,
            "latency_ms": check.latency_ms,
        })).collect::<Vec<_>>(),
    })
}

fn health_monitor_response(monitor: &HealthMonitorStatus) -> Value {
    json!({
        "id": monitor.id,
        "name": monitor.name,
        "service": monitor.service,
        "mode": monitor.mode,
        "expected_every_ms": monitor.expected_every_ms,
        "grace_ms": monitor.grace_ms,
        "state": monitor.state,
        "last_check_in_status": monitor.last_check_in_status,
        "last_check_in_at": monitor.last_check_in_at,
        "last_success_at": monitor.last_success_at,
        "last_failure_at": monitor.last_failure_at,
        "deadline_at": monitor.deadline_at,
    })
}

fn error_group_response(group: &ErrorGroupSummary) -> Value {
    json!({
        "group_hash": group.group_hash,
        "error_class": group.error_class,
        "service": group.service,
        "count": group.total_count,
        "first_seen": group.first_seen,
        "last_seen": group.last_seen,
        "sample_message": group.sample_message,
        "severity": group.severity,
        "status": group.status,
        "classification": {
            "category": group.classification.category,
            "persistence": group.classification.persistence,
            "component": group.classification.component,
        },
    })
}

fn recent_transition_response(transition: &RecentTransition) -> Value {
    json!({
        "entity_type": transition.entity_type,
        "entity_ref": transition.entity_ref,
        "name": transition.name,
        "service": transition.service,
        "state": transition.state,
        "transitioned_at": transition.transitioned_at,
    })
}

fn search_result_response(result: &SearchResult) -> Value {
    json!({
        "id": result.id,
        "service": result.service,
        "error_class": result.error_class,
        "message": result.message,
        "group_hash": result.group_hash,
        "created_at": result.created_at,
        "score": result.score,
    })
}

fn status_target_response(target: &HealthTargetStatus) -> Value {
    json!({
        "id": target.id,
        "name": target.name,
        "url": target.url,
        "state": target.state,
        "consecutive_failures": target.consecutive_failures,
        "last_checked_at": target.last_checked_at,
    })
}

fn status_monitor_response(monitor: &HealthMonitorStatus) -> Value {
    json!({
        "id": monitor.id,
        "name": monitor.name,
        "service": monitor.service,
        "mode": monitor.mode,
        "state": monitor.state,
        "last_check_in_status": monitor.last_check_in_status,
        "last_check_in_at": monitor.last_check_in_at,
        "deadline_at": monitor.deadline_at,
    })
}

fn error_summary_response(item: &ErrorSummaryItem) -> Value {
    json!({
        "service": item.service,
        "total_count": item.total_count,
        "unique_classes": item.unique_classes,
    })
}

fn target_check_response(check: &TargetCheckRead) -> Value {
    json!({
        "checked_at": check.checked_at,
        "result": check.result,
        "status_code": check.status_code,
        "latency_ms": check.latency_ms,
        "tls_expires_at": check.tls_expires_at,
        "error_detail": check.error_detail,
    })
}

fn webhook_response(webhook: WebhookSubscription) -> Value {
    json!({
        "id": webhook.id,
        "url": webhook.url,
        "events": webhook_events(&webhook.events),
        "active": webhook.active,
        "created_at": webhook.created_at,
    })
}

fn webhook_insert_response(webhook: &WebhookSubscriptionInsert) -> Value {
    json!({
        "id": webhook.id,
        "url": webhook.url,
        "events": webhook.events,
        "secret": webhook.secret,
        "created_at": webhook.created_at,
    })
}

fn webhook_events(encoded: &str) -> Vec<String> {
    serde_json::from_str(encoded).unwrap_or_default()
}

fn webhook_endpoint(webhook: WebhookSubscription) -> WebhookEndpoint {
    WebhookEndpoint {
        id: webhook.id,
        url: webhook.url,
        secret: webhook.secret,
        active: webhook.active,
    }
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

fn health_status_summary(
    targets: &[HealthTargetStatus],
    monitors: &[HealthMonitorStatus],
) -> String {
    let mut states = BTreeMap::<&str, Vec<&str>>::new();
    for target in targets {
        states
            .entry(target.state.as_str())
            .or_default()
            .push(target.name.as_str());
    }
    for monitor in monitors {
        states
            .entry(monitor.state.as_str())
            .or_default()
            .push(monitor.name.as_str());
    }

    summarize_health(targets.len() + monitors.len(), &states, "health surfaces")
}

fn combined_overall(
    targets: &[HealthTargetStatus],
    monitors: &[HealthMonitorStatus],
    error_summary: &[ErrorSummaryItem],
) -> String {
    if targets.is_empty() && monitors.is_empty() && error_summary.is_empty() {
        return "empty".to_owned();
    }
    if targets.is_empty() && monitors.is_empty() {
        return "warning".to_owned();
    }

    let has_down = targets.iter().any(|target| target.state == "down")
        || monitors.iter().any(|monitor| monitor.state == "down");
    let has_non_up = targets.iter().any(|target| target.state != "up")
        || monitors.iter().any(|monitor| monitor.state != "up");

    if has_down {
        "unhealthy".to_owned()
    } else if has_non_up {
        "degraded".to_owned()
    } else if !error_summary.is_empty() {
        "warning".to_owned()
    } else {
        "healthy".to_owned()
    }
}

fn combined_status_summary(
    overall: &str,
    targets: &[HealthTargetStatus],
    monitors: &[HealthMonitorStatus],
    error_summary: &[ErrorSummaryItem],
    window: &str,
) -> String {
    let surface_count = targets.len() + monitors.len();
    if overall == "empty" {
        return "No services configured.".to_owned();
    }
    if overall == "healthy" {
        return format!(
            "All {surface_count} health surfaces healthy. No errors in the last {}.",
            window_label(window)
        );
    }

    let mut states = BTreeMap::<&str, Vec<&str>>::new();
    for target in targets {
        states
            .entry(target.state.as_str())
            .or_default()
            .push(target.name.as_str());
    }
    for monitor in monitors {
        states
            .entry(monitor.state.as_str())
            .or_default()
            .push(monitor.name.as_str());
    }
    let total_errors = error_summary
        .iter()
        .map(|item| item.total_count)
        .sum::<i64>();

    let mut summary = format!("{surface_count} health surfaces monitored.");
    if let Some(part) = describe_status_state_group(states.get("down"), "down") {
        summary.push_str(&part);
    }
    if let Some(part) = describe_status_state_group(states.get("degraded"), "degraded") {
        summary.push_str(&part);
    }
    if total_errors > 0 {
        let service_count = error_summary.len();
        let service_word = if service_count == 1 {
            "service"
        } else {
            "services"
        };
        summary.push_str(&format!(
            " {total_errors} errors across {service_count} {service_word} in the last {}.",
            window_label(window)
        ));
    }

    summary
}

fn summarize_health(total: usize, states: &BTreeMap<&str, Vec<&str>>, label: &str) -> String {
    let up = states.get("up").map_or(0, Vec::len);
    let mut parts = vec![format!("{total} {label} monitored. {up} up")];
    if let Some(part) = describe_health_state_group(states.get("degraded"), "degraded") {
        parts.push(part);
    }
    if let Some(part) = describe_health_state_group(states.get("down"), "down") {
        parts.push(part);
    }

    format!("{}.", parts.join(", "))
}

fn describe_health_state_group(names: Option<&Vec<&str>>, label: &str) -> Option<String> {
    let names = names?;
    if names.is_empty() {
        return None;
    }
    Some(format!("{} {label} ({})", names.len(), names.join(", ")))
}

fn describe_status_state_group(names: Option<&Vec<&str>>, label: &str) -> Option<String> {
    describe_health_state_group(names, label).map(|part| format!(" {part}."))
}

fn window_label(window: &str) -> &'static str {
    match window {
        "1h" => "hour",
        "6h" => "6 hours",
        "24h" => "24 hours",
        "7d" => "7 days",
        "30d" => "30 days",
        _ => "requested window",
    }
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

    let mut errors: ValidationErrors = BTreeMap::new();
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
        return Err(Box::new(annotation_missing_fields_problem(errors)));
    }
    if unified_route
        && !canary_store::annotation_subject_types()
            .iter()
            .any(|allowed| *allowed == subject_type)
    {
        let mut errors = BTreeMap::new();
        errors.insert(
            "subject_type".to_owned(),
            vec!["must be one of incident, error_group, target, monitor".to_owned()],
        );
        return Err(Box::new(
            ProblemDetails::new(
                422,
                ProblemCode::ValidationError,
                "Unknown subject_type.",
                None,
            )
            .with_extra("errors", json!(errors)),
        ));
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

fn parse_webhook_create(
    attrs: Map<String, Value>,
) -> Result<WebhookSubscriptionInsert, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = BTreeMap::new();
    let url = required_string(&attrs, "url", &mut errors);
    let events = required_string_array(&attrs, "events", &mut errors);

    if !errors.is_empty() {
        return Err(Box::new(webhook_validation_problem(
            "Invalid webhook configuration.",
        )));
    }

    let Some(url) = url else {
        return Err(Box::new(webhook_validation_problem(
            "Invalid webhook configuration.",
        )));
    };
    let Some(events) = events else {
        return Err(Box::new(webhook_validation_problem(
            "Invalid webhook configuration.",
        )));
    };

    let invalid = events
        .iter()
        .filter(|event| !canary_core::webhook_events::valid(event))
        .cloned()
        .collect::<Vec<_>>();
    if !invalid.is_empty() {
        return Err(Box::new(webhook_validation_problem(format!(
            "Invalid event types: {}",
            invalid.join(", ")
        ))));
    }

    Ok(WebhookSubscriptionInsert {
        id: canary_core::ids::WebhookId::generate().into_string(),
        url,
        events,
        secret: canary_core::secrets::webhook_secret(),
        active: true,
        created_at: current_rfc3339(),
    })
}

fn parse_api_key_create(attrs: Map<String, Value>) -> Result<ApiKeyCreate, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = BTreeMap::new();
    for field in attrs.keys() {
        if !matches!(field.as_str(), "name" | "scope") {
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
    if !matches!(scope.as_str(), "admin" | "ingest-only" | "read-only")
        && !matches!(attrs.get("scope"), Some(value) if !value.is_string())
    {
        errors.insert("scope".to_owned(), vec!["is invalid".to_owned()]);
    }

    if errors.is_empty() {
        Ok(ApiKeyCreate { name, scope })
    } else {
        Err(Box::new(api_key_validation_problem(errors)))
    }
}

fn parse_service_onboarding_create(
    attrs: Map<String, Value>,
    configured_allow_private: bool,
) -> Result<ServiceOnboardingCreate, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = BTreeMap::new();
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
        return Err(Box::new(service_onboarding_validation_problem(errors)));
    }

    let Some(service) = service else {
        return Err(Box::new(service_onboarding_validation_problem(
            BTreeMap::new(),
        )));
    };
    let Some(url) = url else {
        return Err(Box::new(service_onboarding_validation_problem(
            BTreeMap::new(),
        )));
    };
    if let Err(reason) =
        validate_target_configuration(&url, "GET", None, configured_allow_private || allow_private)
    {
        return Err(Box::new(service_onboarding_validation_problem(
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
            Some(value) => Some(value),
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

fn parse_monitor_create(attrs: Map<String, Value>) -> Result<MonitorInsert, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = BTreeMap::new();
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
        return Err(Box::new(monitor_validation_problem(errors)));
    }

    let Some(name) = name else {
        return Err(Box::new(monitor_validation_problem(BTreeMap::new())));
    };
    let Some(mode) = mode else {
        return Err(Box::new(monitor_validation_problem(BTreeMap::new())));
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

fn parse_target_create(
    attrs: Map<String, Value>,
    configured_allow_private: bool,
) -> Result<TargetInsert, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = BTreeMap::new();
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
        return Err(Box::new(target_validation_problem(errors)));
    }

    let Some(name) = name else {
        return Err(Box::new(target_validation_problem(BTreeMap::new())));
    };
    let Some(url) = url else {
        return Err(Box::new(target_validation_problem(BTreeMap::new())));
    };
    let service = optional_string(attrs.get("service")).unwrap_or_else(|| name.clone());
    let allow_private = configured_allow_private || optional_bool(attrs.get("allow_private"));
    if let Err(reason) =
        validate_target_configuration(&url, &method, headers.as_deref(), allow_private)
    {
        return Err(Box::new(ProblemDetails::new(
            422,
            ProblemCode::ValidationError,
            format!("Invalid URL: {reason}"),
            None,
        )));
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
    let mut errors: ValidationErrors = BTreeMap::new();
    if attrs.is_empty() {
        errors.insert(
            "interval_ms".to_owned(),
            vec!["is required for target interval updates".to_owned()],
        );
        return Err(Box::new(target_validation_problem(errors)));
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
        Err(Box::new(target_validation_problem(errors)))
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

fn optional_positive_i64(
    attrs: &Map<String, Value>,
    key: &str,
    default: i64,
    errors: &mut ValidationErrors,
) -> i64 {
    match attrs.get(key) {
        Some(Value::Number(number)) => match number.as_i64().filter(|value| *value > 0) {
            Some(value) => value,
            None => {
                errors.insert(key.to_owned(), vec!["must be greater than 0".to_owned()]);
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

fn optional_positive_u32(
    attrs: &Map<String, Value>,
    key: &str,
    default: u32,
    errors: &mut ValidationErrors,
) -> u32 {
    match attrs.get(key) {
        Some(Value::Number(number)) => match number.as_u64().and_then(|value| {
            if value > 0 {
                u32::try_from(value).ok()
            } else {
                None
            }
        }) {
            Some(value) => value,
            None => {
                errors.insert(key.to_owned(), vec!["must be greater than 0".to_owned()]);
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

fn required_string_array(
    attrs: &Map<String, Value>,
    key: &str,
    errors: &mut ValidationErrors,
) -> Option<Vec<String>> {
    match attrs.get(key) {
        Some(Value::Array(values)) => {
            let mut strings = Vec::new();
            for (index, value) in values.iter().enumerate() {
                match value {
                    Value::String(event) if !event.is_empty() => strings.push(event.clone()),
                    _ => {
                        errors.insert(
                            format!("{key}.{index}"),
                            vec!["must be a non-empty string".to_owned()],
                        );
                    }
                }
            }
            if errors
                .keys()
                .any(|field| field.starts_with(&format!("{key}.")))
            {
                None
            } else {
                Some(strings)
            }
        }
        _ => {
            errors.insert(key.to_owned(), vec!["must be an array".to_owned()]);
            None
        }
    }
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

fn optional_bool(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Bool(true)))
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
    canary_core::health::state_machine::HealthState::parse_persisted(value)
        .ok_or_else(|| format!("unknown health state: {value}"))
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
    if let Err(problem) = require_read_scope(&state, &headers) {
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
    if let Err(problem) = require_read_scope(&state, &headers) {
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
    if let Err(problem) = require_read_scope(&state, &headers) {
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
    let key = require_scope(state, headers, Permission::Ingest)?;
    enforce_rate_limit(state, RateLimitKind::Ingest, &key.id)
}

fn require_read_scope(state: &IngestState, headers: &HeaderMap) -> Result<(), Box<ProblemDetails>> {
    let key = require_scope(state, headers, Permission::Read)?;
    enforce_rate_limit(state, RateLimitKind::Query, &key.id)
}

fn require_query_limited_admin_scope(
    state: &IngestState,
    headers: &HeaderMap,
) -> Result<(), Box<ProblemDetails>> {
    let key = require_scope(state, headers, Permission::Admin)?;
    enforce_rate_limit(state, RateLimitKind::Query, &key.id)
}

fn require_scope(
    state: &IngestState,
    headers: &HeaderMap,
    permission: Permission,
) -> Result<VerifiedApiKey, Box<ProblemDetails>> {
    let authorization_headers = headers
        .get_all(AUTHORIZATION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();

    let token = match extract_bearer(&authorization_headers) {
        BearerToken::Present(token) => token,
        BearerToken::Missing => return Err(Box::new(missing_authorization_problem(None))),
    };

    let store = state.store.lock().map_err(|_| {
        Box::new(ProblemDetails::new(
            500,
            ProblemCode::InternalError,
            "An unexpected error occurred.",
            None,
        ))
    })?;
    let Some(key) = store.verify_api_key(token).map_err(|_| {
        Box::new(ProblemDetails::new(
            500,
            ProblemCode::InternalError,
            "An unexpected error occurred.",
            None,
        ))
    })?
    else {
        return Err(Box::new(invalid_api_key_problem(None)));
    };
    drop(store);

    let Some(scope) = ApiKeyScope::parse(&key.scope) else {
        return Err(Box::new(invalid_api_key_problem(None)));
    };
    if scope.allows(permission) {
        Ok(key)
    } else {
        Err(Box::new(insufficient_scope_problem(
            scope, permission, None,
        )))
    }
}

fn enforce_rate_limit(
    state: &IngestState,
    kind: RateLimitKind,
    identity: &str,
) -> Result<(), Box<ProblemDetails>> {
    let mut limiter = state.rate_limiter.lock().map_err(|_| {
        Box::new(ProblemDetails::new(
            500,
            ProblemCode::InternalError,
            "An unexpected error occurred.",
            None,
        ))
    })?;

    match limiter.check(kind, identity) {
        RateLimitDecision::Allowed => Ok(()),
        RateLimitDecision::Limited {
            retry_after_seconds,
        } => Err(Box::new(rate_limited_problem(retry_after_seconds))),
    }
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

fn decode_optional_json_object(body: &Bytes) -> Result<Map<String, Value>, Box<ProblemDetails>> {
    if body.is_empty() {
        Ok(Map::new())
    } else {
        decode_json_object(body, None)
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

fn monitor_validation_problem(errors: ValidationErrors) -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid monitor configuration.",
        None,
    )
    .with_extra("errors", json!(errors))
}

fn api_key_validation_problem(errors: ValidationErrors) -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid API key request.",
        None,
    )
    .with_extra("errors", json!(errors))
}

fn webhook_validation_problem(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, detail, None)
}

fn webhook_delivery_failed_problem(reason: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(
        502,
        ProblemCode::Other("webhook_delivery_failed".to_owned()),
        format!("Webhook test delivery failed: {}", reason.into()),
        None,
    )
}

fn target_validation_problem(errors: ValidationErrors) -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid target configuration.",
        None,
    )
    .with_extra("errors", json!(errors))
}

fn service_onboarding_validation_problem(errors: ValidationErrors) -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid service onboarding request.",
        None,
    )
    .with_extra("errors", json!(errors))
}

fn service_onboarding_conflict_problem(conflict: TargetConflict) -> ProblemDetails {
    let mut errors: ValidationErrors = BTreeMap::new();
    if conflict.service {
        errors.insert(
            "service".to_owned(),
            vec!["already has a health target".to_owned()],
        );
    }
    if conflict.url {
        errors.insert("url".to_owned(), vec!["is already monitored".to_owned()]);
    }

    service_onboarding_validation_problem(errors)
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

fn invalid_limit_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid limit. Expected a positive integer up to 200.",
        None,
    )
    .with_extra(
        "errors",
        json!({"limit": ["must be a positive integer no greater than 200"]}),
    )
}

fn invalid_cursor_problem() -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, "Invalid cursor.", None).with_extra(
        "errors",
        json!({"cursor": ["must be a valid pagination cursor"]}),
    )
}

fn annotation_missing_fields_problem(errors: ValidationErrors) -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Missing required fields.",
        None,
    )
    .with_extra("errors", json!(errors))
}

fn invalid_annotation_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid annotation.",
        None,
    )
}

fn annotation_missing_subject_problem(field: &str) -> ProblemDetails {
    let mut errors: ValidationErrors = BTreeMap::new();
    errors.insert(field.to_owned(), vec!["is required".to_owned()]);
    ProblemDetails::new(422, ProblemCode::ValidationError, "Missing subject.", None)
        .with_extra("errors", json!(errors))
}

fn invalid_annotation_subject_type_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Unknown subject_type.",
        None,
    )
    .with_extra(
        "errors",
        json!({"subject_type": ["must be one of incident, error_group, target, monitor"]}),
    )
}

fn invalid_annotation_limit_problem() -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, "Invalid limit.", None).with_extra(
        "errors",
        json!({"limit": ["must be an integer between 1 and 50"]}),
    )
}

fn annotation_invalid_cursor_problem() -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, "Invalid cursor.", None)
        .with_extra("errors", json!({"cursor": ["is invalid"]}))
}

fn invalid_event_type_problem(invalid: &[String]) -> ProblemDetails {
    let allowed = canary_core::webhook_events::business().join(", ");
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        format!(
            "Invalid event_type: {}. Allowed: {allowed}",
            invalid.join(", ")
        ),
        None,
    )
    .with_extra(
        "errors",
        json!({"event_type": [format!("must be one or more of: {allowed}")]}),
    )
}

fn invalid_webhook_delivery_status_problem() -> ProblemDetails {
    let allowed = canary_store::webhook_delivery_statuses().join(", ");
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        format!("Invalid status. Allowed: {allowed}"),
        None,
    )
    .with_extra(
        "errors",
        json!({"status": [format!("must be one of: {allowed}")]}),
    )
}

fn invalid_string_param_problem(param: &str) -> ProblemDetails {
    let mut errors: ValidationErrors = BTreeMap::new();
    errors.insert(param.to_owned(), vec!["must be a string".to_owned()]);

    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        format!("Invalid {param} parameter. Must be a string."),
        None,
    )
    .with_extra("errors", json!(errors))
}

fn invalid_report_limit_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid limit. Expected a positive integer.",
        None,
    )
    .with_extra("errors", json!({"limit": ["must be a positive integer"]}))
}

fn invalid_report_cursor_problem() -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, "Invalid cursor.", None).with_extra(
        "errors",
        json!({"cursor": ["must be a valid pagination cursor"]}),
    )
}

fn parse_report_limit(limit: Option<&str>) -> Result<Option<usize>, Box<ProblemDetails>> {
    match limit {
        None => Ok(None),
        Some(raw) => match raw.parse::<usize>() {
            Ok(value) if value > 0 => Ok(Some(value)),
            _ => Err(Box::new(invalid_report_limit_problem())),
        },
    }
}

fn parse_report_cursor(cursor: Option<&str>) -> Result<ReportCursor, Box<ProblemDetails>> {
    match cursor {
        None => Ok(ReportCursor::default()),
        Some(cursor) => {
            decode_report_cursor(cursor).ok_or_else(|| Box::new(invalid_report_cursor_problem()))
        }
    }
}

fn paginate_report_items<T: Clone>(
    items: Vec<T>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> (Vec<T>, Option<usize>) {
    let Some(offset) = offset else {
        return (Vec::new(), None);
    };
    let remaining = items.into_iter().skip(offset).collect::<Vec<_>>();
    let Some(limit) = limit else {
        return (remaining, None);
    };
    let page = remaining.iter().take(limit).cloned().collect::<Vec<_>>();
    let next_offset = if remaining.len() > page.len() {
        Some(offset + page.len())
    } else {
        None
    };
    (page, next_offset)
}

fn accepts_csv(headers: &HeaderMap) -> bool {
    headers
        .get(HeaderName::from_static("accept"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().starts_with("text/csv"))
        })
}

const REPORT_CSV_HEADERS: [&str; 17] = [
    "section",
    "position",
    "id",
    "name",
    "service",
    "error_class",
    "url",
    "state",
    "count",
    "first_seen",
    "last_seen",
    "severity",
    "status",
    "consecutive_failures",
    "last_checked_at",
    "cursor",
    "truncated",
];

fn report_csv(report: &Value) -> String {
    let mut rows = vec![REPORT_CSV_HEADERS.map(str::to_owned).to_vec()];
    rows.extend(report_csv_targets(report));
    rows.extend(report_csv_monitors(report));
    rows.extend(report_csv_error_groups(report));
    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| csv_value(&value))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn report_csv_targets(report: &Value) -> Vec<Vec<String>> {
    report
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, target)| {
            vec![
                "targets".to_owned(),
                (index + 1).to_string(),
                csv_field(target, "id"),
                csv_field(target, "name"),
                String::new(),
                String::new(),
                csv_field(target, "url"),
                csv_field(target, "state"),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                csv_field(target, "consecutive_failures"),
                csv_field(target, "last_checked_at"),
                csv_field(report, "cursor"),
                csv_field(report, "truncated"),
            ]
        })
        .collect()
}

fn report_csv_monitors(report: &Value) -> Vec<Vec<String>> {
    report
        .get("monitors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, monitor)| {
            vec![
                "monitors".to_owned(),
                (index + 1).to_string(),
                csv_field(monitor, "id"),
                csv_field(monitor, "name"),
                csv_field(monitor, "service"),
                String::new(),
                String::new(),
                csv_field(monitor, "state"),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                csv_field(monitor, "last_check_in_status"),
                String::new(),
                csv_field(monitor, "last_check_in_at"),
                csv_field(report, "cursor"),
                csv_field(report, "truncated"),
            ]
        })
        .collect()
}

fn report_csv_error_groups(report: &Value) -> Vec<Vec<String>> {
    report
        .get("error_groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, group)| {
            vec![
                "error_groups".to_owned(),
                (index + 1).to_string(),
                String::new(),
                String::new(),
                csv_field(group, "service"),
                csv_field(group, "error_class"),
                String::new(),
                String::new(),
                csv_field(group, "count"),
                csv_field(group, "first_seen"),
                csv_field(group, "last_seen"),
                csv_field(group, "severity"),
                csv_field(group, "status"),
                String::new(),
                String::new(),
                csv_field(report, "cursor"),
                csv_field(report, "truncated"),
            ]
        })
        .collect()
}

fn csv_field(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn csv_value(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn query_param_is_array(raw_query: Option<&str>, param: &str) -> bool {
    let Some(raw_query) = raw_query else {
        return false;
    };
    let bracket = format!("{param}[]");
    let encoded_bracket = format!("{param}%5B%5D");
    let mut seen = 0;

    for pair in raw_query.split('&') {
        let key = pair.split_once('=').map_or(pair, |(key, _)| key);
        if key == param {
            seen += 1;
            if seen > 1 {
                return true;
            }
        }
        if key == bracket || key.eq_ignore_ascii_case(&encoded_bracket) {
            return true;
        }
    }

    false
}

fn target_checks_window_problem() -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, "Invalid window.", None)
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
        API_KEY_PREFIX_LEN, ApiKeyInsert, MonitorInsert, TargetCheckObservation, TargetProbeCommit,
        WebhookDeliveryInsert, WebhookDeliveryJobInsert, WebhookDeliveryJobState,
        WebhookDeliveryStatus, WebhookSubscriptionInsert,
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
    async fn monitor_check_in_records_enqueue_failures_without_changing_response()
    -> Result<(), Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        seed_api_key(&mut store, "KEY-ingest", INGEST_KEY, "ingest-only", None)?;
        seed_monitor(&mut store, "desktop-active-timer")?;
        let store = Arc::new(Mutex::new(store));
        let recorder = Arc::new(EnqueueFailureRecorder::default());
        let state = IngestState::new_with_shared_fanout(
            store,
            IngestConfig::default(),
            Arc::new(NoopIngestEffectSink),
            HealthEventFanout::new(Arc::new(FailingEventSink), recorder.clone()),
        );

        let response = ingest_router(state)
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"desktop-active-timer","status":"alive","observed_at":"2026-05-28T20:00:00Z"}"#,
            )?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);
        let snapshot = recorder.snapshot();
        assert_eq!(
            snapshot.get(&EnqueueFailureKey {
                source: HealthEventSource::MonitorCheckIn,
                event: "health_check.recovered".to_owned(),
            }),
            Some(&1)
        );

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
    async fn admin_target_mutations_emit_lifecycle_commands() -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?
            .with_target_control(recorder.clone())
            .with_allow_private_targets(true);
        let router = ingest_router(state);

        let create_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/targets",
                ADMIN_KEY,
                r#"{
                    "url":"http://127.0.0.1:9/health",
                    "name":"Local API",
                    "service":"local-api",
                    "interval_ms":2500,
                    "timeout_ms":1000,
                    "allow_private":true
                }"#,
            )?)
            .await?;
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let created = json_body(create_response).await?;
        let target_id = created["id"]
            .as_str()
            .ok_or("missing target id")?
            .to_owned();
        assert_eq!(created["service"], "local-api");
        assert_eq!(created["active"], true);

        let list_response = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        let list_body = json_body(list_response).await?;
        assert!(
            list_body["targets"]
                .as_array()
                .ok_or("targets should be an array")?
                .iter()
                .any(|target| target["id"] == target_id)
        );

        let pause_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/targets/{target_id}/pause"),
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(pause_response.status(), StatusCode::OK);
        assert_eq!(json_body(pause_response).await?["status"], "paused");

        let resume_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/targets/{target_id}/resume"),
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(resume_response.status(), StatusCode::OK);
        assert_eq!(json_body(resume_response).await?["status"], "resumed");

        let delete_response = router
            .clone()
            .oneshot(
                Request::delete(format!("/api/v1/targets/{target_id}"))
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

        assert_eq!(
            recorder.commands(),
            vec![
                TargetProbeLifecycleCommand::Track {
                    target_id: target_id.clone(),
                    interval_ms: 2500,
                },
                TargetProbeLifecycleCommand::Pause {
                    target_id: target_id.clone(),
                },
                TargetProbeLifecycleCommand::Resume {
                    target_id: target_id.clone(),
                },
                TargetProbeLifecycleCommand::Untrack { target_id },
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn service_onboarding_creates_target_ingest_key_and_snippets()
    -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?.with_target_control(recorder.clone());
        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(json_request_with_host(
                "POST",
                "/api/v1/service-onboarding",
                ADMIN_KEY,
                "www.example.com",
                r#"{
                    "service":" billing api ",
                    "url":"https://example.com/billing/health",
                    "environment":" staging ",
                    "interval_ms":30000
                }"#,
            )?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);
        let created = json_body(response).await?;
        let target_id = created["target"]["id"]
            .as_str()
            .ok_or("missing target id")?
            .to_owned();
        let raw_key = created["api_key"]["key"]
            .as_str()
            .ok_or("missing raw ingest key")?
            .to_owned();

        assert_eq!(created["service"], "billing api");
        assert_eq!(created["api_key"]["name"], "billing api-ingest");
        assert_eq!(created["api_key"]["scope"], "ingest-only");
        assert!(raw_key.starts_with("sk_live_"));
        assert_eq!(
            created["api_key"]["key_prefix"],
            raw_key.chars().take(API_KEY_PREFIX_LEN).collect::<String>()
        );
        assert_eq!(created["target"]["name"], "billing api");
        assert_eq!(created["target"]["service"], "billing api");
        assert_eq!(
            created["target"]["url"],
            "https://example.com/billing/health"
        );
        assert_eq!(created["target"]["method"], "GET");
        assert_eq!(created["target"]["interval_ms"], 30_000);
        assert_eq!(created["target"]["timeout_ms"], 10_000);
        assert_eq!(created["target"]["expected_status"], "200");
        assert_eq!(created["target"]["active"], true);
        assert_eq!(
            created["links"]["report"],
            "http://www.example.com/api/v1/report?window=1h"
        );
        assert_eq!(
            created["links"]["service_query"],
            "http://www.example.com/api/v1/query?service=billing+api&window=1h"
        );
        assert!(
            created["snippets"]["error_ingest_curl"]
                .as_str()
                .ok_or("missing ingest snippet")?
                .contains(&raw_key)
        );
        assert!(
            created["snippets"]["typescript_init"]
                .as_str()
                .ok_or("missing typescript snippet")?
                .contains("service: \"billing api\"")
        );

        let ingest_response = router
            .clone()
            .oneshot(error_request(
                &raw_key,
                r#"{"service":"billing api","environment":"staging","error_class":"RuntimeError","message":"canary onboarding check"}"#,
            )?)
            .await?;
        assert_eq!(ingest_response.status(), StatusCode::CREATED);

        assert_eq!(
            recorder.commands(),
            vec![TargetProbeLifecycleCommand::Track {
                target_id,
                interval_ms: 30_000,
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn service_onboarding_rejects_invalid_scope_shape_and_conflicts_without_writes()
    -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?
            .with_target_control(recorder.clone())
            .with_allow_private_targets(true);
        let router = ingest_router(state);

        let forbidden_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/service-onboarding",
                INGEST_KEY,
                r#"{"service":"worker","url":"http://127.0.0.1:9/health","allow_private":true}"#,
            )?)
            .await?;
        assert_eq!(forbidden_response.status(), StatusCode::FORBIDDEN);
        let targets_after_forbidden = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        assert_eq!(
            json_body(targets_after_forbidden).await?["targets"],
            json!([])
        );
        assert!(recorder.commands().is_empty());

        let invalid_url_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/service-onboarding",
                ADMIN_KEY,
                r#"{"service":"worker","url":"ftp://example.com/health"}"#,
            )?)
            .await?;
        assert_eq!(
            invalid_url_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let invalid_url = json_body(invalid_url_response).await?;
        assert_eq!(invalid_url["detail"], "Invalid service onboarding request.");
        assert_eq!(
            invalid_url["errors"]["url"],
            json!(["scheme must be http or https"])
        );

        let create_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/service-onboarding",
                ADMIN_KEY,
                r#"{"service":"worker","url":"http://127.0.0.1:9/health","allow_private":true}"#,
            )?)
            .await?;
        assert_eq!(create_response.status(), StatusCode::CREATED);

        let duplicate_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/service-onboarding",
                ADMIN_KEY,
                r#"{"service":"worker","url":"http://127.0.0.1:10/health","allow_private":true}"#,
            )?)
            .await?;
        assert_eq!(
            duplicate_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let duplicate = json_body(duplicate_response).await?;
        assert_eq!(
            duplicate["errors"]["service"],
            json!(["already has a health target"])
        );

        let keys_response = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/keys")?)
            .await?;
        let key_count = json_body(keys_response).await?["keys"]
            .as_array()
            .ok_or("keys should be an array")?
            .len();
        assert_eq!(key_count, 5);

        let targets_response = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        let target_count = json_body(targets_response).await?["targets"]
            .as_array()
            .ok_or("targets should be an array")?
            .len();
        assert_eq!(target_count, 1);

        Ok(())
    }

    #[tokio::test]
    async fn admin_target_interval_update_reconfigures_only_when_cadence_changes()
    -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?
            .with_target_control(recorder.clone())
            .with_allow_private_targets(true);
        let router = ingest_router(state);

        let create_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/targets",
                ADMIN_KEY,
                r#"{
                    "url":"http://127.0.0.1:9/health",
                    "name":"Local API",
                    "interval_ms":2500,
                    "allow_private":true
                }"#,
            )?)
            .await?;
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let target_id = json_body(create_response).await?["id"]
            .as_str()
            .ok_or("missing target id")?
            .to_owned();

        let update_response = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/v1/targets/{target_id}"),
                ADMIN_KEY,
                r#"{"interval_ms":5000}"#,
            )?)
            .await?;
        assert_eq!(update_response.status(), StatusCode::OK);
        let updated = json_body(update_response).await?;
        assert_eq!(updated["interval_ms"], 5000);
        assert_eq!(updated["active"], true);

        let unchanged_response = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/v1/targets/{target_id}"),
                ADMIN_KEY,
                r#"{"interval_ms":5000}"#,
            )?)
            .await?;
        assert_eq!(unchanged_response.status(), StatusCode::OK);

        let pause_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/targets/{target_id}/pause"),
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(pause_response.status(), StatusCode::OK);

        let inactive_update_response = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/v1/targets/{target_id}"),
                ADMIN_KEY,
                r#"{"interval_ms":7500}"#,
            )?)
            .await?;
        assert_eq!(inactive_update_response.status(), StatusCode::OK);

        assert_eq!(
            recorder.commands(),
            vec![
                TargetProbeLifecycleCommand::Track {
                    target_id: target_id.clone(),
                    interval_ms: 2500,
                },
                TargetProbeLifecycleCommand::Reconfigure {
                    target_id: target_id.clone(),
                    interval_ms: 5000,
                },
                TargetProbeLifecycleCommand::Pause { target_id },
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn admin_target_interval_update_rejects_invalid_scope_and_shape()
    -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?
            .with_target_control(recorder.clone())
            .with_allow_private_targets(true);
        let router = ingest_router(state);

        let create_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/targets",
                ADMIN_KEY,
                r#"{
                    "url":"http://127.0.0.1:9/health",
                    "name":"Local API",
                    "allow_private":true
                }"#,
            )?)
            .await?;
        let target_id = json_body(create_response).await?["id"]
            .as_str()
            .ok_or("missing target id")?
            .to_owned();

        let forbidden_response = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/v1/targets/{target_id}"),
                INGEST_KEY,
                r#"{"interval_ms":5000}"#,
            )?)
            .await?;
        assert_eq!(forbidden_response.status(), StatusCode::FORBIDDEN);

        let empty_response = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/v1/targets/{target_id}"),
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(empty_response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let unsupported_response = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/v1/targets/{target_id}"),
                ADMIN_KEY,
                r#"{"name":"New Name"}"#,
            )?)
            .await?;
        assert_eq!(
            unsupported_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let missing_response = router
            .oneshot(json_request(
                "PATCH",
                "/api/v1/targets/TGT-missing",
                ADMIN_KEY,
                r#"{"interval_ms":5000}"#,
            )?)
            .await?;
        assert_eq!(missing_response.status(), StatusCode::NOT_FOUND);

        assert_eq!(
            recorder.commands(),
            vec![TargetProbeLifecycleCommand::Track {
                target_id,
                interval_ms: 60000,
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn admin_target_create_rejects_ingest_scope_without_writing_or_commanding()
    -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?
            .with_target_control(recorder.clone())
            .with_allow_private_targets(true);
        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/targets",
                INGEST_KEY,
                r#"{"url":"http://127.0.0.1:9/health","name":"Local API","allow_private":true}"#,
            )?)
            .await?;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(recorder.commands().is_empty());
        let list = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        assert_eq!(json_body(list).await?["targets"], json!([]));

        Ok(())
    }

    #[tokio::test]
    async fn admin_monitor_mutations_follow_phoenix_contract() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        let create_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/monitors",
                ADMIN_KEY,
                r#"{"name":"desktop-active-timer","mode":"ttl","expected_every_ms":90000}"#,
            )?)
            .await?;
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let created = json_body(create_response).await?;
        let monitor_id = created["id"]
            .as_str()
            .ok_or("missing monitor id")?
            .to_owned();
        assert!(monitor_id.starts_with("MON-"));
        assert_eq!(created["name"], "desktop-active-timer");
        assert_eq!(created["service"], "desktop-active-timer");
        assert_eq!(created["mode"], "ttl");
        assert_eq!(created["expected_every_ms"], 90_000);
        assert_eq!(created["grace_ms"], 0);
        assert!(created["created_at"].as_str().is_some());

        let list_response = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/monitors")?)
            .await?;
        assert_eq!(list_response.status(), StatusCode::OK);
        let listed = json_body(list_response).await?;
        assert!(
            listed["monitors"]
                .as_array()
                .ok_or("monitors should be an array")?
                .iter()
                .any(|monitor| monitor["id"] == monitor_id)
        );

        let delete_response = router
            .clone()
            .oneshot(
                Request::delete(format!("/api/v1/monitors/{monitor_id}"))
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

        let missing_response = router
            .clone()
            .oneshot(
                Request::delete(format!("/api/v1/monitors/{monitor_id}"))
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(missing_response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(missing_response).await?["detail"],
            "Monitor not found."
        );

        let final_list = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/monitors")?)
            .await?;
        assert_eq!(json_body(final_list).await?["monitors"], json!([]));

        Ok(())
    }

    #[tokio::test]
    async fn admin_monitor_create_rejects_invalid_scope_and_shape() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        let forbidden_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/monitors",
                INGEST_KEY,
                r#"{"name":"worker","mode":"ttl","expected_every_ms":90000}"#,
            )?)
            .await?;
        assert_eq!(forbidden_response.status(), StatusCode::FORBIDDEN);
        let list_after_forbidden = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/monitors")?)
            .await?;
        assert_eq!(
            json_body(list_after_forbidden).await?["monitors"],
            json!([])
        );

        let invalid_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/monitors",
                ADMIN_KEY,
                r#"{"name":"worker","mode":"sometimes","expected_every_ms":0,"grace_ms":-1}"#,
            )?)
            .await?;
        assert_eq!(invalid_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let invalid_body = json_body(invalid_response).await?;
        assert_eq!(invalid_body["code"], "validation_error");
        assert_eq!(invalid_body["detail"], "Invalid monitor configuration.");
        assert_eq!(
            invalid_body["errors"]["mode"],
            json!(["must be one of: schedule, ttl"])
        );
        assert_eq!(
            invalid_body["errors"]["expected_every_ms"],
            json!(["must be greater than 0"])
        );
        assert_eq!(
            invalid_body["errors"]["grace_ms"],
            json!(["must be greater than or equal to 0"])
        );

        let missing_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/monitors",
                ADMIN_KEY,
                r#"{"name":"worker","mode":"ttl"}"#,
            )?)
            .await?;
        assert_eq!(missing_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(missing_response).await?["errors"]["expected_every_ms"],
            json!(["must be a positive integer"])
        );

        let create_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/monitors",
                ADMIN_KEY,
                r#"{"name":"worker","mode":"ttl","expected_every_ms":90000}"#,
            )?)
            .await?;
        assert_eq!(create_response.status(), StatusCode::CREATED);

        let duplicate_response = router
            .oneshot(json_request(
                "POST",
                "/api/v1/monitors",
                ADMIN_KEY,
                r#"{"name":"worker","mode":"ttl","expected_every_ms":90000}"#,
            )?)
            .await?;
        assert_eq!(
            duplicate_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            json_body(duplicate_response).await?["errors"]["name"],
            json!(["has already been taken"])
        );

        Ok(())
    }

    #[tokio::test]
    async fn admin_webhook_mutations_follow_phoenix_contract() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(
            test_ingest_state()?.with_webhook_transport(Arc::new(RecordingTransport::status(204))),
        );

        let create_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/webhooks",
                ADMIN_KEY,
                r#"{"url":"https://example.com/hook","events":["error.new_class","canary.ping"]}"#,
            )?)
            .await?;
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let created = json_body(create_response).await?;
        let webhook_id = created["id"]
            .as_str()
            .ok_or("missing webhook id")?
            .to_owned();
        assert!(webhook_id.starts_with("WHK-"));
        assert_eq!(created["url"], "https://example.com/hook");
        assert_eq!(created["events"], json!(["error.new_class", "canary.ping"]));
        assert_eq!(
            created["secret"]
                .as_str()
                .ok_or("missing webhook secret")?
                .len(),
            32
        );
        assert!(created["created_at"].as_str().is_some());

        let list_response = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/webhooks")?)
            .await?;
        assert_eq!(list_response.status(), StatusCode::OK);
        let listed = json_body(list_response).await?;
        let listed_webhook = listed["webhooks"]
            .as_array()
            .ok_or("webhooks should be an array")?
            .iter()
            .find(|webhook| webhook["id"] == webhook_id)
            .ok_or("missing listed webhook")?;
        assert_eq!(listed_webhook["url"], "https://example.com/hook");
        assert_eq!(
            listed_webhook["events"],
            json!(["error.new_class", "canary.ping"])
        );
        assert_eq!(listed_webhook["active"], true);
        assert!(listed_webhook.get("secret").is_none());

        let test_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/webhooks/{webhook_id}/test"),
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(test_response.status(), StatusCode::OK);
        assert_eq!(json_body(test_response).await?["status"], "delivered");

        let delete_response = router
            .clone()
            .oneshot(
                Request::delete(format!("/api/v1/webhooks/{webhook_id}"))
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

        let missing_response = router
            .clone()
            .oneshot(
                Request::delete(format!("/api/v1/webhooks/{webhook_id}"))
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(missing_response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(missing_response).await?["detail"],
            "Webhook not found."
        );

        let final_list = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/webhooks")?)
            .await?;
        assert_eq!(json_body(final_list).await?["webhooks"], json!([]));

        Ok(())
    }

    #[tokio::test]
    async fn admin_webhook_test_delivery_uses_blocking_transport_boundary()
    -> Result<(), Box<dyn Error>> {
        let transport = Arc::new(ThreadRecordingTransport::status(500));
        let router = ingest_router(test_ingest_state()?.with_webhook_transport(transport.clone()));
        let caller_thread = thread::current().id();

        let create_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/webhooks",
                ADMIN_KEY,
                r#"{"url":"https://example.com/hook","events":["canary.ping"]}"#,
            )?)
            .await?;
        let webhook_id = json_body(create_response).await?["id"]
            .as_str()
            .ok_or("missing webhook id")?
            .to_owned();

        let failed_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/webhooks/{webhook_id}/test"),
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(failed_response.status(), StatusCode::BAD_GATEWAY);
        let body = json_body(failed_response).await?;
        assert_eq!(body["code"], "webhook_delivery_failed");
        assert_eq!(body["detail"], "Webhook test delivery failed: HTTP 500");
        assert!(
            transport
                .thread_ids()?
                .iter()
                .all(|thread_id| *thread_id != caller_thread)
        );

        let missing_response = router
            .oneshot(json_request(
                "POST",
                "/api/v1/webhooks/WHK-missing/test",
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(missing_response.status(), StatusCode::NOT_FOUND);

        Ok(())
    }

    #[tokio::test]
    async fn admin_webhook_create_rejects_invalid_scope_and_events() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        let forbidden_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/webhooks",
                INGEST_KEY,
                r#"{"url":"https://example.com/hook","events":["error.new_class"]}"#,
            )?)
            .await?;
        assert_eq!(forbidden_response.status(), StatusCode::FORBIDDEN);
        let list_after_forbidden = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/webhooks")?)
            .await?;
        assert_eq!(
            json_body(list_after_forbidden).await?["webhooks"],
            json!([])
        );

        let invalid_event_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/webhooks",
                ADMIN_KEY,
                r#"{"url":"https://example.com/hook","events":["bogus.event"]}"#,
            )?)
            .await?;
        assert_eq!(
            invalid_event_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let invalid_event = json_body(invalid_event_response).await?;
        assert_eq!(invalid_event["code"], "validation_error");
        assert_eq!(invalid_event["detail"], "Invalid event types: bogus.event");

        let invalid_shape_response = router
            .oneshot(json_request(
                "POST",
                "/api/v1/webhooks",
                ADMIN_KEY,
                r#"{"url":"","events":["error.new_class",7]}"#,
            )?)
            .await?;
        assert_eq!(
            invalid_shape_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            json_body(invalid_shape_response).await?["detail"],
            "Invalid webhook configuration."
        );

        Ok(())
    }

    #[tokio::test]
    async fn admin_api_key_mutations_follow_phoenix_contract() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        let create_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/keys",
                ADMIN_KEY,
                r#"{"name":"deploy","scope":"read-only"}"#,
            )?)
            .await?;
        assert_eq!(create_response.status(), StatusCode::CREATED);
        let created = json_body(create_response).await?;
        let key_id = created["id"].as_str().ok_or("missing key id")?.to_owned();
        let raw_key = created["key"].as_str().ok_or("missing raw key")?.to_owned();
        assert!(key_id.starts_with("KEY-"));
        assert!(raw_key.starts_with("sk_live_"));
        assert_eq!(created["name"], "deploy");
        assert_eq!(created["scope"], "read-only");
        assert_eq!(
            created["key_prefix"],
            &raw_key[..canary_store::API_KEY_PREFIX_LEN]
        );
        assert_eq!(
            created["warning"],
            "Store this key securely. It will not be shown again."
        );
        assert!(created["created_at"].as_str().is_some());

        let list_response = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/keys")?)
            .await?;
        assert_eq!(list_response.status(), StatusCode::OK);
        let listed = json_body(list_response).await?;
        let listed_key = listed["keys"]
            .as_array()
            .ok_or("keys should be an array")?
            .iter()
            .find(|key| key["id"] == key_id)
            .ok_or("missing listed key")?;
        assert_eq!(listed_key["name"], "deploy");
        assert_eq!(listed_key["scope"], "read-only");
        assert_eq!(
            listed_key["key_prefix"],
            &raw_key[..canary_store::API_KEY_PREFIX_LEN]
        );
        assert_eq!(listed_key["active"], true);
        assert_eq!(listed_key["revoked_at"], Value::Null);
        assert!(listed_key.get("key").is_none());
        assert!(listed_key.get("key_hash").is_none());

        let read_with_created_key = router
            .clone()
            .oneshot(read_request(&raw_key, "/api/v1/incidents")?)
            .await?;
        assert_eq!(read_with_created_key.status(), StatusCode::OK);

        let revoke_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/keys/{key_id}/revoke"),
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(revoke_response.status(), StatusCode::OK);
        assert_eq!(json_body(revoke_response).await?["status"], "revoked");

        let list_after_revoke = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/keys")?)
            .await?;
        let revoked_key = json_body(list_after_revoke).await?["keys"]
            .as_array()
            .ok_or("keys should be an array")?
            .iter()
            .find(|key| key["id"] == key_id)
            .ok_or("missing revoked key")?
            .clone();
        assert_eq!(revoked_key["active"], false);
        assert!(revoked_key["revoked_at"].as_str().is_some());

        let read_with_revoked_key = router
            .clone()
            .oneshot(read_request(&raw_key, "/api/v1/incidents")?)
            .await?;
        assert_eq!(read_with_revoked_key.status(), StatusCode::UNAUTHORIZED);

        let missing_response = router
            .oneshot(json_request(
                "POST",
                "/api/v1/keys/KEY-missing/revoke",
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(missing_response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(missing_response).await?["detail"],
            "API key not found."
        );

        Ok(())
    }

    #[tokio::test]
    async fn admin_api_key_create_defaults_and_rejects_invalid_scope() -> Result<(), Box<dyn Error>>
    {
        let router = ingest_router(test_ingest_state()?);

        let default_response = router
            .clone()
            .oneshot(
                Request::post("/api/v1/keys")
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(default_response.status(), StatusCode::CREATED);
        let default_key = json_body(default_response).await?;
        assert_eq!(default_key["name"], "unnamed");
        assert_eq!(default_key["scope"], "admin");

        let forbidden_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/keys",
                INGEST_KEY,
                r#"{"name":"bad","scope":"admin"}"#,
            )?)
            .await?;
        assert_eq!(forbidden_response.status(), StatusCode::FORBIDDEN);

        let invalid_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/keys",
                ADMIN_KEY,
                r#"{"name":7,"scope":"super-admin"}"#,
            )?)
            .await?;
        assert_eq!(invalid_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let invalid_body = json_body(invalid_response).await?;
        assert_eq!(invalid_body["detail"], "Invalid API key request.");
        assert_eq!(invalid_body["errors"]["name"], json!(["must be a string"]));
        assert_eq!(invalid_body["errors"]["scope"], json!(["is invalid"]));

        let blank_name_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/keys",
                ADMIN_KEY,
                r#"{"name":"","scope":"admin"}"#,
            )?)
            .await?;
        assert_eq!(
            blank_name_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let blank_name_body = json_body(blank_name_response).await?;
        assert_eq!(blank_name_body["errors"]["name"], json!(["can't be blank"]));

        let extra_field_response = router
            .oneshot(json_request(
                "POST",
                "/api/v1/keys",
                ADMIN_KEY,
                r#"{"name":"extra-key","scope":"admin","extra":true}"#,
            )?)
            .await?;
        assert_eq!(
            extra_field_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let extra_field_body = json_body(extra_field_response).await?;
        assert_eq!(
            extra_field_body["errors"]["extra"],
            json!(["is not permitted"])
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
    async fn timeline_accepts_read_scope_filters_and_paginates() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);
        for body in [
            r#"{"service":"alpha","error_class":"RuntimeError","message":"first"}"#,
            r#"{"service":"alpha","error_class":"ArgumentError","message":"second"}"#,
            r#"{"service":"beta","error_class":"RuntimeError","message":"third"}"#,
        ] {
            let response = router
                .clone()
                .oneshot(json_request("POST", "/api/v1/errors", INGEST_KEY, body)?)
                .await?;
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let unfiltered = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/timeline?event_type=error.new_class",
            )?)
            .await?;
        let unfiltered_status = unfiltered.status();
        let unfiltered_body = json_body(unfiltered).await?;

        assert_eq!(unfiltered_status, StatusCode::OK);
        assert_eq!(unfiltered_body["service"], Value::Null);
        assert_eq!(
            unfiltered_body["summary"],
            "Returned 3 timeline events in the last 24h."
        );

        let first = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/timeline?service=alpha&event_type=error.new_class&limit=1",
            )?)
            .await?;
        let first_status = first.status();
        let first_body = json_body(first).await?;

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(first_body["service"], "alpha");
        assert_eq!(first_body["window"], "24h");
        assert_eq!(first_body["returned_count"], 1);
        assert_eq!(first_body["events"][0]["service"], "alpha");
        assert_eq!(first_body["events"][0]["event"], "error.new_class");
        assert_eq!(
            first_body["events"][0]["payload"]["event"],
            "error.new_class"
        );
        assert!(first_body["cursor"].as_str().is_some());

        let cursor = first_body["cursor"].as_str().ok_or("missing cursor")?;
        let second = router
            .oneshot(read_request(
                READ_KEY,
                &format!(
                    "/api/v1/timeline?service=alpha&event_type=error.new_class&limit=1&after={cursor}&cursor=bogus"
                ),
            )?)
            .await?;
        let second_status = second.status();
        let second_body = json_body(second).await?;

        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(second_body["returned_count"], 1);
        assert_eq!(second_body["events"][0]["service"], "alpha");
        assert_eq!(second_body["cursor"], Value::Null);

        Ok(())
    }

    #[tokio::test]
    async fn timeline_rejects_invalid_params_and_wrong_scope() -> Result<(), Box<dyn Error>> {
        let cases = [
            (
                read_request(INGEST_KEY, "/api/v1/timeline")?,
                StatusCode::FORBIDDEN,
                "insufficient_scope",
                "detail",
                "API key scope `ingest-only` cannot access this read endpoint. Use an `admin` or `read-only` key.",
            ),
            (
                read_request(READ_KEY, "/api/v1/timeline?window=99h")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "window",
                canary_core::query::INVALID_WINDOW_FIELD_ERROR,
            ),
            (
                read_request(READ_KEY, "/api/v1/timeline?limit=201")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "limit",
                "must be a positive integer no greater than 200",
            ),
            (
                read_request(READ_KEY, "/api/v1/timeline?cursor=bogus")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "cursor",
                "must be a valid pagination cursor",
            ),
            (
                read_request(READ_KEY, "/api/v1/timeline?event_type=canary.ping")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "event_type",
                "must be one or more of:",
            ),
        ];

        for (request, expected_status, expected_code, field, expected_fragment) in cases {
            let response = ingest_router(test_ingest_state()?).oneshot(request).await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, expected_status);
            assert_eq!(body["code"], expected_code);
            if field == "detail" {
                assert_eq!(body["detail"], expected_fragment);
            } else {
                assert!(
                    body["errors"][field][0]
                        .as_str()
                        .is_some_and(|error| error.contains(expected_fragment))
                );
            }
        }

        let unauthorized = ingest_router(test_ingest_state()?)
            .oneshot(Request::get("/api/v1/timeline").body(Body::empty())?)
            .await?;
        let unauthorized_status = unauthorized.status();
        let unauthorized_body = json_body(unauthorized).await?;

        assert_eq!(unauthorized_status, StatusCode::UNAUTHORIZED);
        assert_eq!(unauthorized_body["code"], "invalid_api_key");

        Ok(())
    }

    #[tokio::test]
    async fn webhook_deliveries_accept_read_scope_filters_and_paginate()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
            store.create_pending_webhook_delivery(WebhookDeliveryInsert {
                delivery_id: "DLV-old".to_owned(),
                webhook_id: "WHK-alpha".to_owned(),
                event: "error.new_class".to_owned(),
                now: "2026-04-02T10:00:00Z".to_owned(),
            })?;
            store.mark_webhook_delivery_attempt("DLV-old", "2026-04-02T10:00:01Z")?;
            store.mark_webhook_delivery_delivered("DLV-old", "2026-04-02T10:00:02Z")?;
            store.create_suppressed_webhook_delivery(
                WebhookDeliveryInsert {
                    delivery_id: "DLV-suppressed".to_owned(),
                    webhook_id: "WHK-alpha".to_owned(),
                    event: "error.new_class".to_owned(),
                    now: "2026-04-02T10:05:00Z".to_owned(),
                },
                "cooldown",
            )?;
            store.create_suppressed_webhook_delivery(
                WebhookDeliveryInsert {
                    delivery_id: "DLV-other".to_owned(),
                    webhook_id: "WHK-beta".to_owned(),
                    event: "incident.updated".to_owned(),
                    now: "2026-04-02T10:10:00Z".to_owned(),
                },
                "cooldown",
            )?;
            store.create_pending_webhook_delivery(WebhookDeliveryInsert {
                delivery_id: "DLV-pending".to_owned(),
                webhook_id: "WHK-pending".to_owned(),
                event: "error.new_class".to_owned(),
                now: "2026-04-02T10:15:00Z".to_owned(),
            })?;
        }
        let router = ingest_router(state);

        let filtered = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/webhook-deliveries?webhook_id=WHK-alpha&limit=2",
            )?)
            .await?;
        let filtered_status = filtered.status();
        let filtered_body = json_body(filtered).await?;

        assert_eq!(filtered_status, StatusCode::OK);
        assert_eq!(filtered_body["returned_count"], 2);
        assert_eq!(
            filtered_body["deliveries"]
                .as_array()
                .ok_or("deliveries should be array")?
                .iter()
                .map(|delivery| delivery["delivery_id"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["DLV-suppressed", "DLV-old"]
        );
        assert_eq!(filtered_body["cursor"], Value::Null);
        assert_eq!(filtered_body["deliveries"][0]["status"], "suppressed");
        assert_eq!(filtered_body["deliveries"][0]["reason"], "cooldown");
        assert_eq!(
            filtered_body["deliveries"][0]["completed_at"],
            "2026-04-02T10:05:00Z"
        );
        assert_eq!(filtered_body["deliveries"][1]["status"], "delivered");
        assert_eq!(
            filtered_body["deliveries"][1]["delivered_at"],
            "2026-04-02T10:00:02Z"
        );
        assert_eq!(
            filtered_body["deliveries"][1]["completed_at"],
            "2026-04-02T10:00:02Z"
        );

        let pending = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/webhook-deliveries?webhook_id=WHK-pending",
            )?)
            .await?;
        let pending_body = json_body(pending).await?;
        let pending_delivery = &pending_body["deliveries"][0];
        assert_eq!(pending_delivery["delivery_id"], "DLV-pending");
        for field in [
            "reason",
            "first_attempt_at",
            "last_attempt_at",
            "delivered_at",
            "discarded_at",
            "completed_at",
        ] {
            assert_eq!(pending_delivery[field], Value::Null);
        }

        let first_page = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/webhook-deliveries?status=suppressed&limit=1",
            )?)
            .await?;
        let first_body = json_body(first_page).await?;
        assert_eq!(first_body["returned_count"], 1);
        assert_eq!(first_body["deliveries"][0]["delivery_id"], "DLV-other");
        let cursor = first_body["cursor"].as_str().ok_or("missing cursor")?;

        let second_page = router
            .oneshot(read_request(
                READ_KEY,
                &format!(
                    "/api/v1/webhook-deliveries?status=suppressed&limit=1&after={cursor}&cursor=bogus"
                ),
            )?)
            .await?;
        let second_status = second_page.status();
        let second_body = json_body(second_page).await?;

        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(
            second_body["deliveries"][0]["delivery_id"],
            "DLV-suppressed"
        );
        assert_eq!(second_body["cursor"], Value::Null);

        Ok(())
    }

    #[tokio::test]
    async fn webhook_deliveries_reject_invalid_params_and_wrong_scope() -> Result<(), Box<dyn Error>>
    {
        let cases = [
            (
                read_request(INGEST_KEY, "/api/v1/webhook-deliveries")?,
                StatusCode::FORBIDDEN,
                "insufficient_scope",
                "detail",
                "API key scope `ingest-only` cannot access this read endpoint. Use an `admin` or `read-only` key.",
            ),
            (
                read_request(READ_KEY, "/api/v1/webhook-deliveries?limit=0")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "limit",
                "must be a positive integer no greater than 200",
            ),
            (
                read_request(READ_KEY, "/api/v1/webhook-deliveries?cursor=bogus")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "cursor",
                "must be a valid pagination cursor",
            ),
            (
                read_request(READ_KEY, "/api/v1/webhook-deliveries?status=supressed")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "status",
                "must be one of: pending, retrying, delivered, discarded, suppressed",
            ),
            (
                read_request(
                    READ_KEY,
                    "/api/v1/webhook-deliveries?status%5B%5D=suppressed",
                )?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "status",
                "must be a string",
            ),
        ];

        for (request, expected_status, expected_code, field, expected_fragment) in cases {
            let response = ingest_router(test_ingest_state()?).oneshot(request).await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, expected_status);
            assert_eq!(body["code"], expected_code);
            if field == "detail" {
                assert_eq!(body["detail"], expected_fragment);
            } else {
                assert!(
                    body["errors"][field][0]
                        .as_str()
                        .is_some_and(|error| error.contains(expected_fragment))
                );
            }
        }

        let unauthorized = ingest_router(test_ingest_state()?)
            .oneshot(Request::get("/api/v1/webhook-deliveries").body(Body::empty())?)
            .await?;
        let unauthorized_status = unauthorized.status();
        let unauthorized_body = json_body(unauthorized).await?;

        assert_eq!(unauthorized_status, StatusCode::UNAUTHORIZED);
        assert_eq!(unauthorized_body["code"], "invalid_api_key");

        Ok(())
    }

    #[tokio::test]
    async fn health_status_accepts_read_scope_and_returns_surfaces() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?.with_allow_private_targets(true);
        {
            let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
            seed_monitor(&mut store, "desktop-active-timer")?;
        }
        let router = ingest_router(state);

        let target_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/targets",
                ADMIN_KEY,
                r#"{
                    "url":"http://127.0.0.1:9/health",
                    "name":"Local API",
                    "service":"local-api",
                    "allow_private":true
                }"#,
            )?)
            .await?;
        assert_eq!(target_response.status(), StatusCode::CREATED);

        let response = router
            .oneshot(read_request(READ_KEY, "/api/v1/health-status")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["summary"], "2 health surfaces monitored. 0 up.");
        assert_eq!(body["targets"][0]["name"], "Local API");
        assert_eq!(body["targets"][0]["service"], "local-api");
        assert_eq!(body["targets"][0]["state"], "unknown");
        assert_eq!(body["targets"][0]["recent_checks"], json!([]));
        assert_eq!(body["monitors"][0]["name"], "desktop-active-timer");
        assert_eq!(body["monitors"][0]["state"], "unknown");
        assert!(body["monitors"][0].get("grace_ms").is_some());

        Ok(())
    }

    #[tokio::test]
    async fn status_defaults_to_empty_without_surfaces_or_errors() -> Result<(), Box<dyn Error>> {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(read_request(READ_KEY, "/api/v1/status")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["overall"], "empty");
        assert_eq!(body["summary"], "No services configured.");
        assert_eq!(body["targets"], json!([]));
        assert_eq!(body["monitors"], json!([]));
        assert_eq!(body["error_summary"], json!([]));

        Ok(())
    }

    #[tokio::test]
    async fn status_combines_error_summary_with_default_window() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state);

        let ingest = router
            .clone()
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        assert_eq!(ingest.status(), StatusCode::CREATED);

        let response = router
            .oneshot(read_request(READ_KEY, "/api/v1/status")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["overall"], "warning");
        assert_eq!(
            body["summary"],
            "0 health surfaces monitored. 1 errors across 1 service in the last hour."
        );
        assert_eq!(body["error_summary"][0]["service"], "test-svc");
        assert_eq!(body["error_summary"][0]["total_count"], 1);
        assert_eq!(body["error_summary"][0]["unique_classes"], 1);

        Ok(())
    }

    #[tokio::test]
    async fn status_rejects_invalid_window_and_missing_auth() -> Result<(), Box<dyn Error>> {
        let invalid = ingest_router(test_ingest_state()?)
            .oneshot(read_request(READ_KEY, "/api/v1/status?window=99h")?)
            .await?;
        let invalid_status = invalid.status();
        let invalid_body = json_body(invalid).await?;

        assert_eq!(invalid_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(invalid_body["code"], "validation_error");
        assert_eq!(
            invalid_body["errors"]["window"],
            json!(["must be one of: 1h, 6h, 24h, 7d, 30d"])
        );

        let unauthorized = ingest_router(test_ingest_state()?)
            .oneshot(Request::get("/api/v1/status").body(Body::empty())?)
            .await?;
        let unauthorized_status = unauthorized.status();
        let unauthorized_body = json_body(unauthorized).await?;

        assert_eq!(unauthorized_status, StatusCode::UNAUTHORIZED);
        assert_eq!(unauthorized_body["code"], "invalid_api_key");

        Ok(())
    }

    #[tokio::test]
    async fn report_accepts_read_scope_searches_paginates_and_renders_csv()
    -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        for service in [
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf",
        ] {
            let response = router
                .clone()
                .oneshot(json_request(
                    "POST",
                    "/api/v1/targets",
                    ADMIN_KEY,
                    &format!(
                        r#"{{"name":"{service}","service":"{service}","url":"https://example.com/{service}/health"}}"#
                    ),
                )?)
                .await?;
            assert_eq!(response.status(), StatusCode::CREATED);
        }
        for service in [
            "svc-a", "svc-b", "svc-c", "svc-d", "svc-e", "svc-f", "svc-g",
        ] {
            let response = router
                .clone()
                .oneshot(json_request(
                    "POST",
                    "/api/v1/errors",
                    INGEST_KEY,
                    &format!(
                        r#"{{"service":"{service}","error_class":"TimeoutError","message":"timeout while reporting {service}"}}"#
                    ),
                )?)
                .await?;
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let first = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/report?window=1h&limit=5&q=timeout",
            )?)
            .await?;
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = json_body(first).await?;
        assert_eq!(first_body["targets"].as_array().map(Vec::len), Some(5));
        assert_eq!(first_body["error_groups"].as_array().map(Vec::len), Some(5));
        assert_eq!(
            first_body["search_results"].as_array().map(Vec::len),
            Some(7)
        );
        assert_eq!(first_body["truncated"], true);
        let cursor = first_body["cursor"]
            .as_str()
            .ok_or("first report should return cursor")?;

        let second = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                &format!("/api/v1/report?window=1h&limit=5&cursor={cursor}"),
            )?)
            .await?;
        let second_body = json_body(second).await?;
        assert_eq!(second_body["targets"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            second_body["error_groups"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(second_body["truncated"], false);
        assert_eq!(second_body["cursor"], Value::Null);

        let exact_page = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/report?window=1h&limit=7")?)
            .await?;
        let exact_page_body = json_body(exact_page).await?;
        assert_eq!(exact_page_body["targets"].as_array().map(Vec::len), Some(7));
        assert_eq!(
            exact_page_body["error_groups"].as_array().map(Vec::len),
            Some(7)
        );
        assert_eq!(exact_page_body["truncated"], false);
        assert_eq!(exact_page_body["cursor"], Value::Null);

        let partial_cursor = encode_report_cursor(&ReportCursor {
            targets_offset: None,
            monitor_offset: None,
            error_groups_offset: Some(5),
        })
        .ok_or("partial report cursor should encode")?;
        let partial_page = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                &format!("/api/v1/report?window=1h&limit=5&cursor={partial_cursor}"),
            )?)
            .await?;
        let partial_page_body = json_body(partial_page).await?;
        assert_eq!(partial_page_body["targets"], json!([]));
        assert_eq!(partial_page_body["monitors"], json!([]));
        assert_eq!(
            partial_page_body["error_groups"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(partial_page_body["truncated"], false);
        assert_eq!(partial_page_body["cursor"], Value::Null);

        let csv = router
            .clone()
            .oneshot(
                Request::get("/api/v1/report?limit=5")
                    .header("authorization", format!("Bearer {READ_KEY}"))
                    .header("accept", "text/csv")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(
            csv.headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/csv; charset=utf-8")
        );
        let csv_body = String::from_utf8(to_bytes(csv.into_body(), usize::MAX).await?.to_vec())?;
        assert!(
            csv_body.starts_with("section,position,id,name,service,error_class,url,state,count")
        );
        assert!(csv_body.contains("targets,1,"));
        assert!(csv_body.contains("error_groups,1,"));

        let invalid_q = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/report?q%5B%5D=timeout")?)
            .await?;
        assert_eq!(invalid_q.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(invalid_q).await?["errors"]["q"],
            json!(["must be a string"])
        );

        let invalid_cursor = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/report?cursor=W10")?)
            .await?;
        assert_eq!(invalid_cursor.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(invalid_cursor).await?["errors"]["cursor"],
            json!(["must be a valid pagination cursor"])
        );

        let invalid_empty_limit = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/report?limit=")?)
            .await?;
        assert_eq!(
            invalid_empty_limit.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            json_body(invalid_empty_limit).await?["errors"]["limit"],
            json!(["must be a positive integer"])
        );

        let forbidden = router
            .oneshot(read_request(INGEST_KEY, "/api/v1/report")?)
            .await?;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

        Ok(())
    }

    #[tokio::test]
    async fn metrics_requires_admin_scope_and_returns_prometheus_snapshot()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
            seed_target(&mut store, "metrics-svc")?;
            seed_monitor(&mut store, "metrics-monitor")?;
            store.create_pending_webhook_delivery(WebhookDeliveryInsert {
                delivery_id: "DLV-metrics".to_owned(),
                webhook_id: "WHK-metrics".to_owned(),
                event: "error.new_class".to_owned(),
                now: "2026-05-28T20:00:00Z".to_owned(),
            })?;
            store.insert_webhook_delivery_job(WebhookDeliveryJobInsert {
                args: json!({"delivery_id": "DLV-metrics"}),
                scheduled_at: "2026-05-28T20:00:00Z".to_owned(),
                now: "2026-05-28T20:00:00Z".to_owned(),
                max_attempts: 20,
            })?;
        }

        let response = ingest_router(state.clone())
            .oneshot(read_request(ADMIN_KEY, "/metrics")?)
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static(PROMETHEUS_CONTENT_TYPE))
        );
        let body = text_body(response).await?;
        assert!(body.contains("# HELP canary_webhook_queue_depth"));
        assert!(body.contains("# TYPE canary_oban_queue_depth gauge"));
        assert!(body.contains("canary_webhook_queue_depth 1"));
        assert!(body.contains("canary_webhook_delivery_total{status=\"pending\"} 1"));
        assert!(body.contains("canary_oban_queue_depth{queue=\"webhooks\"} 1"));
        assert!(body.contains(
            "canary_probe_state{target_id=\"TGT-metrics-svc\",service=\"metrics-svc\",state=\"unknown\"} 1"
        ));
        assert!(body.contains(
            "canary_monitor_state{monitor_id=\"MON-metrics-monitor\",service=\"metrics-monitor\",state=\"unknown\"} 1"
        ));

        let forbidden = ingest_router(state.clone())
            .oneshot(read_request(READ_KEY, "/metrics")?)
            .await?;
        let forbidden_status = forbidden.status();
        let forbidden_body = json_body(forbidden).await?;
        assert_eq!(forbidden_status, StatusCode::FORBIDDEN);
        assert_eq!(forbidden_body["code"], "insufficient_scope");

        let unauthorized = ingest_router(state)
            .oneshot(Request::get("/metrics").body(Body::empty())?)
            .await?;
        let unauthorized_status = unauthorized.status();
        let unauthorized_body = json_body(unauthorized).await?;
        assert_eq!(unauthorized_status, StatusCode::UNAUTHORIZED);
        assert_eq!(unauthorized_body["code"], "invalid_api_key");

        Ok(())
    }

    #[tokio::test]
    async fn target_checks_accepts_read_scope_and_returns_recent_checks()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?.with_allow_private_targets(true);
        let router = ingest_router(state.clone());

        let target_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/targets",
                ADMIN_KEY,
                r#"{
                    "url":"http://127.0.0.1:9/health",
                    "name":"Local API",
                    "service":"local-api",
                    "allow_private":true
                }"#,
            )?)
            .await?;
        assert_eq!(target_response.status(), StatusCode::CREATED);
        let target = json_body(target_response).await?;
        let target_id = target["id"].as_str().ok_or("missing target id")?.to_owned();
        {
            let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
            store.commit_target_probe(TargetProbeCommit {
                target_id: target_id.clone(),
                state: "up".to_owned(),
                consecutive_failures: 0,
                consecutive_successes: 1,
                check_succeeded: true,
                check: TargetCheckObservation {
                    status_code: Some(200),
                    latency_ms: Some(42),
                    result: "ok".to_owned(),
                    tls_expires_at: Some("2026-09-01T00:00:00Z".to_owned()),
                    error_detail: None,
                    region: None,
                },
                now: current_rfc3339(),
                transition: None,
            })?;
        }

        let response = router
            .oneshot(read_request(
                READ_KEY,
                &format!("/api/v1/targets/{target_id}/checks"),
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["target_id"], target_id);
        assert_eq!(body["window"], "24h");
        assert_eq!(body["checks"][0]["result"], "ok");
        assert_eq!(body["checks"][0]["status_code"], 200);
        assert_eq!(body["checks"][0]["latency_ms"], 42);
        assert_eq!(body["checks"][0]["tls_expires_at"], "2026-09-01T00:00:00Z");
        assert_eq!(body["checks"][0]["error_detail"], Value::Null);

        Ok(())
    }

    #[tokio::test]
    async fn target_checks_keeps_phoenix_error_and_empty_missing_target_behavior()
    -> Result<(), Box<dyn Error>> {
        let missing = ingest_router(test_ingest_state()?)
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/targets/TGT-missing/checks",
            )?)
            .await?;
        let missing_status = missing.status();
        let missing_body = json_body(missing).await?;

        assert_eq!(missing_status, StatusCode::OK);
        assert_eq!(missing_body["target_id"], "TGT-missing");
        assert_eq!(missing_body["window"], "24h");
        assert_eq!(missing_body["checks"], json!([]));

        let unauthorized = ingest_router(test_ingest_state()?)
            .oneshot(Request::get("/api/v1/targets/TGT-any/checks").body(Body::empty())?)
            .await?;
        let unauthorized_status = unauthorized.status();
        let unauthorized_body = json_body(unauthorized).await?;

        assert_eq!(unauthorized_status, StatusCode::UNAUTHORIZED);
        assert_eq!(unauthorized_body["code"], "invalid_api_key");

        let cases = [
            (
                read_request(READ_KEY, "/api/v1/targets/TGT-any/checks?window=99h")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "Invalid window.",
            ),
            (
                read_request(INGEST_KEY, "/api/v1/targets/TGT-any/checks")?,
                StatusCode::FORBIDDEN,
                "insufficient_scope",
                "API key scope `ingest-only` cannot access this read endpoint. Use an `admin` or `read-only` key.",
            ),
        ];

        for (request, expected_status, expected_code, expected_detail) in cases {
            let response = ingest_router(test_ingest_state()?).oneshot(request).await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, expected_status);
            assert_eq!(body["code"], expected_code);
            assert_eq!(body["detail"], expected_detail);
        }

        Ok(())
    }

    #[tokio::test]
    async fn annotations_create_list_paginate_and_emit_webhook_effect() -> Result<(), Box<dyn Error>>
    {
        let sink = Arc::new(RecordingFailingSink::default());
        let state = test_ingest_state_with_sink(sink.clone())?;
        {
            let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
            seed_target(&mut store, "api")?;
        }
        let router = ingest_router(state);

        let alpha = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/annotations",
                ADMIN_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-api","agent":"alpha","action":"paged","metadata":{"ticket":"OPS-1"}}"#,
            )?)
            .await?;
        let alpha_status = alpha.status();
        let alpha_body = json_body(alpha).await?;
        assert_eq!(alpha_status, StatusCode::CREATED);
        assert!(
            alpha_body["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("ANN-"))
        );
        assert_eq!(alpha_body["subject_type"], "target");
        assert_eq!(alpha_body["subject_id"], "TGT-api");
        assert_eq!(alpha_body["incident_id"], Value::Null);
        assert_eq!(alpha_body["group_hash"], Value::Null);
        assert_eq!(alpha_body["metadata"]["ticket"], "OPS-1");

        thread::sleep(StdDuration::from_millis(2));
        let beta = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/annotations",
                ADMIN_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-api","agent":"beta","action":"silenced"}"#,
            )?)
            .await?;
        let beta_status = beta.status();
        let beta_body = json_body(beta).await?;
        assert_eq!(beta_status, StatusCode::CREATED);
        assert_eq!(beta_body["metadata"], Value::Null);

        {
            let effects = sink.effects.lock().map_err(|_| "effect lock poisoned")?;
            assert_eq!(effects.len(), 2);
            match &effects[0] {
                IngestEffect::EnqueueWebhook {
                    event,
                    payload_json,
                } => {
                    assert_eq!(event, "annotation.added");
                    let payload: Value = serde_json::from_str(payload_json)?;
                    assert_eq!(
                        payload,
                        json!({
                            "event": "annotation.added",
                            "annotation": {
                                "id": alpha_body["id"],
                                "subject_type": "target",
                                "subject_id": "TGT-api",
                                "incident_id": null,
                                "group_hash": null,
                                "agent": "alpha",
                                "action": "paged",
                                "metadata": {"ticket": "OPS-1"},
                                "created_at": alpha_body["created_at"],
                            },
                            "timestamp": alpha_body["created_at"],
                        })
                    );
                }
                other => return Err(format!("unexpected effect: {other:?}").into()),
            }
        }

        let page1 = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/annotations?subject_type=target&subject_id=TGT-api&limit=1",
            )?)
            .await?;
        let page1_body = json_body(page1).await?;
        assert_eq!(page1_body["annotations"].as_array().map(Vec::len), Some(1));
        assert_eq!(page1_body["annotations"][0]["agent"], "beta");
        assert!(
            page1_body["summary"]
                .as_str()
                .is_some_and(|s| s.contains("2 annotations"))
        );
        let cursor = page1_body["cursor"]
            .as_str()
            .ok_or("missing annotation cursor")?;

        let page2 = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                &format!(
                    "/api/v1/annotations?subject_type=target&subject_id=TGT-api&limit=1&cursor={cursor}"
                ),
            )?)
            .await?;
        let page2_body = json_body(page2).await?;
        assert_eq!(page2_body["annotations"].as_array().map(Vec::len), Some(1));
        assert_eq!(page2_body["annotations"][0]["agent"], "alpha");
        assert_eq!(page2_body["cursor"], Value::Null);

        Ok(())
    }

    #[tokio::test]
    async fn legacy_annotation_routes_and_errors_follow_phoenix_contract()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state.clone());
        let created_error = router
            .clone()
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        let body = json_body(created_error).await?;
        let group_hash = body["group_hash"]
            .as_str()
            .ok_or("missing group hash")?
            .to_owned();
        let incident_id = {
            let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
            let id = canary_core::ids::IncidentId::generate().into_string();
            store.correlate_incident(IncidentCorrelation {
                signal_type: "error_group".to_owned(),
                signal_ref: group_hash,
                service: "test-svc".to_owned(),
                incident_id: id.parse()?,
                event_id: canary_core::ids::EventId::generate(),
                now: "2026-05-28T20:00:00Z".to_owned(),
            })?;
            id
        };

        let created = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/annotations"),
                ADMIN_KEY,
                r#"{"agent":"triage-bot","action":"acknowledged"}"#,
            )?)
            .await?;
        let created_status = created.status();
        let created_body = json_body(created).await?;
        assert_eq!(created_status, StatusCode::CREATED);
        assert_eq!(created_body["incident_id"], incident_id);
        assert_eq!(created_body["group_hash"], Value::Null);
        assert_eq!(created_body["subject_type"], "incident");
        assert_eq!(created_body["subject_id"], incident_id);

        let listed = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                &format!("/api/v1/incidents/{incident_id}/annotations"),
            )?)
            .await?;
        let listed_body = json_body(listed).await?;
        assert_eq!(listed_body["annotations"].as_array().map(Vec::len), Some(1));
        assert_eq!(listed_body["annotations"][0]["agent"], "triage-bot");

        let forbidden_legacy = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/annotations"),
                READ_KEY,
                r#"{"agent":"bot","action":"ack"}"#,
            )?)
            .await?;
        assert_eq!(forbidden_legacy.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            json_body(forbidden_legacy).await?["code"],
            "insufficient_scope"
        );

        let missing_field = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/annotations",
                ADMIN_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-api","action":"ack"}"#,
            )?)
            .await?;
        let missing_status = missing_field.status();
        let missing_body = json_body(missing_field).await?;
        assert_eq!(missing_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(missing_body["errors"]["agent"], json!(["is required"]));

        let invalid_type = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/annotations",
                ADMIN_KEY,
                r#"{"subject_type":"incident","subject_id":"INC-x","agent":123,"action":"ack"}"#,
            )?)
            .await?;
        let invalid_type_body = json_body(invalid_type).await?;
        assert_eq!(invalid_type_body["code"], "validation_error");
        assert_eq!(invalid_type_body["detail"], "Invalid annotation.");
        assert!(invalid_type_body.get("errors").is_none());

        let bad_subject = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/annotations?subject_type=spaceship&subject_id=X-1",
            )?)
            .await?;
        assert_eq!(bad_subject.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(bad_subject).await?["errors"]["subject_type"],
            json!(["must be one of incident, error_group, target, monitor"])
        );

        let forbidden = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/annotations",
                READ_KEY,
                r#"{"subject_type":"incident","subject_id":"INC-x","agent":"bot","action":"ack"}"#,
            )?)
            .await?;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(json_body(forbidden).await?["code"], "insufficient_scope");

        let invalid_cursor = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                &format!("/api/v1/annotations?subject_type=incident&subject_id={incident_id}&cursor=bogus"),
            )?)
            .await?;
        assert_eq!(invalid_cursor.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(invalid_cursor).await?["errors"]["cursor"],
            json!(["is invalid"])
        );

        let invalid_limit = router
            .oneshot(read_request(
                READ_KEY,
                &format!(
                    "/api/v1/annotations?subject_type=incident&subject_id={incident_id}&limit=51"
                ),
            )?)
            .await?;
        assert_eq!(invalid_limit.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(invalid_limit).await?["errors"]["limit"],
            json!(["must be an integer between 1 and 50"])
        );

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

    #[tokio::test]
    async fn ingest_and_query_routes_enforce_phoenix_rate_limit_buckets()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut limiter = state
                .rate_limiter
                .lock()
                .map_err(|_| "rate limiter lock poisoned")?;
            for _ in 0..100 {
                assert_eq!(
                    limiter.check(RateLimitKind::Ingest, "KEY-ingest"),
                    RateLimitDecision::Allowed
                );
            }
            for _ in 0..30 {
                assert_eq!(
                    limiter.check(RateLimitKind::Query, "KEY-read"),
                    RateLimitDecision::Allowed
                );
            }
        }
        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(error_request(INGEST_KEY, "{}")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], "rate_limited");
        let retry_after = body["retry_after"]
            .as_u64()
            .ok_or("retry_after should be a number")?;
        assert!((1..=60).contains(&retry_after));

        let response = router
            .oneshot(read_request(READ_KEY, "/api/v1/incidents")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], "rate_limited");
        let retry_after = body["retry_after"]
            .as_u64()
            .ok_or("retry_after should be a number")?;
        assert!((1..=60).contains(&retry_after));

        Ok(())
    }

    async fn json_body(response: Response<Body>) -> Result<Value, Box<dyn Error>> {
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice(&bytes)?;

        Ok(body)
    }

    async fn text_body(response: Response<Body>) -> Result<String, Box<dyn Error>> {
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        Ok(String::from_utf8(bytes.to_vec())?)
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

    fn json_request(
        method: &'static str,
        path: &str,
        token: &str,
        body: &str,
    ) -> Result<Request<Body>, Box<dyn Error>> {
        Ok(Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header(CONTENT_TYPE, APPLICATION_JSON)
            .body(Body::from(body.to_owned()))?)
    }

    fn json_request_with_host(
        method: &'static str,
        path: &str,
        token: &str,
        host: &str,
        body: &str,
    ) -> Result<Request<Body>, Box<dyn Error>> {
        Ok(Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("host", host)
            .header(CONTENT_TYPE, APPLICATION_JSON)
            .body(Body::from(body.to_owned()))?)
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

    fn seed_target(store: &mut Store, service: &str) -> Result<(), Box<dyn Error>> {
        store.insert_target(TargetInsert {
            id: format!("TGT-{service}"),
            url: format!("https://example.com/{service}/health"),
            name: service.to_owned(),
            service: service.to_owned(),
            method: "GET".to_owned(),
            headers: None,
            interval_ms: 60_000,
            timeout_ms: 10_000,
            expected_status: "200".to_owned(),
            body_contains: None,
            degraded_after: 1,
            down_after: 3,
            up_after: 1,
            active: true,
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

    struct FailingEventSink;

    impl EventSink for FailingEventSink {
        fn enqueue_event(&self, event: &str, _payload_json: &str) -> Result<(), String> {
            Err(format!("simulated enqueue failure for {event}"))
        }
    }

    #[derive(Default)]
    struct RecordingTargetControl {
        commands: StdMutex<Vec<TargetProbeLifecycleCommand>>,
    }

    impl RecordingTargetControl {
        fn commands(&self) -> Vec<TargetProbeLifecycleCommand> {
            match self.commands.lock() {
                Ok(commands) => commands.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }
    }

    impl TargetControlSink for RecordingTargetControl {
        fn control_target(&self, command: TargetProbeLifecycleCommand) -> Result<(), String> {
            match self.commands.lock() {
                Ok(mut commands) => commands.push(command),
                Err(poisoned) => poisoned.into_inner().push(command),
            }
            Ok(())
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
