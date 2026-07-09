//! Axum server wiring for Canary.
//!
//! This crate adapts the stable wire contracts from `canary-http` to concrete
//! HTTP responses. Domain decisions and body shapes stay out of the router.

use axum::{
    Router,
    http::{
        Method,
        header::{AUTHORIZATION, CONTENT_TYPE, HeaderName},
    },
    routing::{delete, get, patch, post},
};
#[cfg(test)]
use canary_http::rate_limit::RateLimitKind;
#[cfg(test)]
use canary_store::IncidentCorrelation;

mod admin_keys;
mod admin_monitors;
mod admin_targets;
mod admin_webhooks;
mod annotations;
mod auth_cache;
mod body_fields;
mod claims;
mod dashboard_routes;
mod egress;
mod health_fanout;
mod health_routes;
mod http_contract;
mod incident_escalation;
mod ingest_routes;
pub mod keygen;
mod metrics_routes;
mod monitor_overdue;
mod public_routes;
mod query_routes;
mod rate_limit;
mod read_audit;
mod read_source;
mod report_routes;
mod responder_context;
mod retention_prune;
mod route_state;
mod runtime;
mod runtime_env;
mod server_auth;
mod server_time;
mod service_onboarding_routes;
mod target_probes;
mod target_request;
mod telemetry_routes;
mod tls_scan;
mod webhook_delivery;
mod webhook_delivery_routes;
mod webhooks;
mod worker_health;

use admin_keys::{create_api_key, list_api_keys, revoke_api_key};
use admin_monitors::{create_monitor, delete_monitor, list_monitors};
use admin_targets::{
    create_target, delete_target, list_targets, pause_target, resume_target, update_target_interval,
};
use admin_webhooks::{create_webhook, delete_webhook, list_webhooks, test_webhook};
use annotations::{
    create_annotation, create_group_annotation, create_incident_annotation, list_annotations,
    list_group_annotations, list_incident_annotations,
};
use claims::{create_claim, list_claims, release_claim, show_claim, transition_claim};
pub use dashboard_routes::dashboard_router;
pub use health_fanout::{
    EnqueueFailure, EnqueueFailureKey, EnqueueFailureRecorder, EnqueueFailureSink,
    EventFanoutReport, HealthEventFanout, HealthEventSource,
};
use health_routes::{health_status, status, target_checks};
use incident_escalation::{deescalate_incident, escalate_incident};
use ingest_routes::{create_check_in, create_error};
use metrics_routes::metrics;
pub use monitor_overdue::{
    MonitorOverdueLifecycle, MonitorOverdueLifecycleConfig, MonitorOverdueLifecycleReport,
    MonitorOverdueLifecycleWorker, MonitorOverdueOutcome, MonitorOverdueRuntime,
    MonitorOverdueRuntimeError, run_monitor_overdue_once,
};
pub use public_routes::{
    PublicReadiness, PublicReadinessProbe, PublicReadinessSnapshot, public_router,
};
use query_routes::{list_incidents, query_errors, show_error, show_incident, timeline};
#[cfg(test)]
use rate_limit::RateLimitDecision;
use rate_limit::RateLimiter;
use report_routes::report;
pub use retention_prune::{
    RetentionPruneLifecycle, RetentionPruneLifecycleConfig, RetentionPruneLifecycleReport,
    RetentionPruneLifecycleWorker,
};
pub use route_state::{
    AuthFailIdentityConfig, EventSink, IngestEffectSink, IngestState, TargetControlSink,
};
pub use runtime::{
    CanaryServer, RuntimeIngestEffectSink, ServerBootError, ServerConfig, ServerRunError,
};
pub use runtime_env::{RuntimeEnvError, ServerProcessConfig};
#[cfg(test)]
pub(crate) use server_auth::{UNKNOWN_AUTH_FAIL_IDENTITY, auth_fail_identity};
pub(crate) use server_auth::{
    enforce_service_authority, require_ingest_scope, require_query_limited_admin_scope,
    require_read_scope, require_responder_write_scope, require_scope, service_authority_problem,
};
use service_onboarding_routes::create_service_onboarding;
pub use target_probes::{
    ProbeHttpResponse, ProbeRequest, ProbeTransport, ProbeTransportError, ReqwestProbeTransport,
    TargetProbeLifecycle, TargetProbeLifecycleCommand, TargetProbeLifecycleConfig,
    TargetProbeLifecycleController, TargetProbeLifecycleReport, TargetProbeLifecycleWorker,
    TargetProbeOptions, TargetProbeOutcome, TargetProbeRuntime, TargetProbeRuntimeError,
    run_target_probe_once, validate_target_configuration, validate_target_probe_interval_ms,
};
use telemetry_routes::create_event;
pub use tls_scan::{
    TlsExpiryScanLifecycle, TlsExpiryScanLifecycleConfig, TlsExpiryScanLifecycleReport,
    TlsExpiryScanLifecycleWorker, TlsExpiryScanRuntimeError, run_tls_expiry_scan_once,
};
use tower_http::{
    catch_panic::CatchPanicLayer,
    cors::{Any, CorsLayer},
};
pub use webhook_delivery::{
    InMemoryWebhookCircuit, WebhookCircuit, WebhookDeliveryDrain, WebhookDeliveryDrainReport,
    WebhookDeliveryDrainWorker, WebhookDeliveryRuntime,
};
use webhook_delivery_routes::{webhook_deliveries, webhook_delivery};
pub use webhooks::{
    HttpWebhookTransport, InMemoryWebhookCooldown, StoreWebhookScheduler, WebhookCooldown,
    WebhookEnqueueEffectSink, WebhookScheduler, WebhookTransport,
};
pub(crate) use worker_health::{
    WorkerHealthHandle, WorkerHealthRegistry, WorkerName, WorkerPressureSnapshot,
};

/// Router for Canary's authenticated ingest endpoints.
///
/// Wrapped in [`CatchPanicLayer`] so one panicking handler yields a single
/// RFC 9457 500 for that request instead of an aborted connection. Combined
/// with the writer store's non-poisoning `parking_lot::Mutex`
/// (`route_state::SharedStore`), a panic while holding the store lock can no
/// longer wedge every subsequent authenticated request behind a poisoned
/// mutex (canary-930: "request path must not poison the writer mutex").
pub fn ingest_router(state: IngestState) -> Router {
    Router::<IngestState>::new()
        .route("/metrics", get(metrics))
        .route(
            "/api/v1/errors",
            post(create_error).layer(browser_ingest_cors_layer()),
        )
        .route(
            "/api/v1/events",
            post(create_event).layer(browser_ingest_cors_layer()),
        )
        .route("/api/v1/check-ins", post(create_check_in))
        .route("/api/v1/query", get(query_errors))
        .route("/api/v1/report", get(report))
        .route("/api/v1/timeline", get(timeline))
        .route("/api/v1/webhook-deliveries", get(webhook_deliveries))
        .route(
            "/api/v1/webhook-deliveries/{delivery_id}",
            get(webhook_delivery),
        )
        .route("/api/v1/status", get(status))
        .route("/api/v1/health-status", get(health_status))
        .route("/api/v1/targets/{id}/checks", get(target_checks))
        .route("/api/v1/incidents", get(list_incidents))
        .route("/api/v1/incidents/{id}", get(show_incident))
        .route("/api/v1/incidents/{id}/escalate", post(escalate_incident))
        .route(
            "/api/v1/incidents/{id}/deescalate",
            post(deescalate_incident),
        )
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
        .route("/api/v1/claims", get(list_claims).post(create_claim))
        .route("/api/v1/claims/{id}", get(show_claim))
        .route("/api/v1/claims/{id}/transition", post(transition_claim))
        .route("/api/v1/claims/{id}/release", post(release_claim))
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
        .layer(CatchPanicLayer::custom(handle_request_panic))
}

fn browser_ingest_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::POST, Method::OPTIONS])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE])
}

/// Convert a panic unwinding out of a request handler into one RFC 9457 500
/// response. Applied to the whole authenticated router (`ingest_router`) so
/// no single handler bug can turn into a dropped connection or -- combined
/// with the non-poisoning writer mutex -- a stuck process (canary-930).
fn handle_request_panic(
    err: Box<dyn std::any::Any + Send + 'static>,
) -> axum::http::Response<axum::body::Body> {
    let message = if let Some(message) = err.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = err.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "unknown panic".to_owned()
    };
    tracing::error!(
        panic.message = %message,
        "request handler panicked; converted to one 500 response"
    );
    http_contract::problem_response(canary_http::problem_details::internal_problem())
}

/// Headers set by the public adapter.
pub const PUBLIC_CONTENT_TYPE: HeaderName = CONTENT_TYPE;

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::process;
    use std::str::FromStr;
    use std::sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };
    use std::thread::{self, JoinHandle, ThreadId};
    use std::time::{Duration as StdDuration, Instant};

    use crate::route_state::SharedStore;
    use axum::{
        body::{Body, to_bytes},
        http::{
            HeaderMap, HeaderValue, Method, Request, Response, StatusCode,
            header::{CACHE_CONTROL, CONTENT_TYPE},
        },
    };
    use canary_core::{
        ids::{ErrorId, EventId, IncidentId},
        ingest::classification::{Category, Classification, Component, Persistence},
    };
    use canary_http::{
        public::{
            APPLICATION_JSON, DependencyStatus, OPENAPI_JSON, WorkerHealthStatus,
            WorkerLifecycleState, WorkerPressureShape, WorkerReadyzCheck,
        },
        request::MAX_JSON_BODY_BYTES,
    };
    use canary_ingest::{IngestConfig, IngestEffect};
    use canary_store::{
        API_KEY_PREFIX_LEN, AnnotationInsert, ApiKeyInsert, ErrorIngest, ErrorIngestIds,
        ErrorIngestPayload, MonitorCheckInCommit, MonitorCheckInObservation, MonitorInsert,
        ReadPool, Store, TargetCheckObservation, TargetInsert, TargetProbeCommit,
        WebhookDeliveryInsert, WebhookDeliveryJobCompletion, WebhookDeliveryJobInsert,
        WebhookDeliveryJobState, WebhookDeliveryStatus, WebhookSubscriptionInsert,
    };
    use canary_workers::{
        retention::RetentionPolicy,
        webhooks::{CircuitDecision, TransportResult, WebhookJob, WebhookRequest},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    #[derive(Debug, Default)]
    struct TestNoopIngestEffectSink;

    impl IngestEffectSink for TestNoopIngestEffectSink {
        fn handle(&self, _effects: &[IngestEffect]) -> Result<(), String> {
            Ok(())
        }
    }

    struct ToggleReadinessProbe {
        ready: AtomicBool,
    }

    impl PublicReadinessProbe for ToggleReadinessProbe {
        fn snapshot(&self) -> PublicReadinessSnapshot {
            let database = if self.ready.load(Ordering::SeqCst) {
                DependencyStatus::Ok
            } else {
                DependencyStatus::Error
            };
            PublicReadinessSnapshot::new(database, DependencyStatus::Ok)
        }
    }

    struct StaticSnapshotProbe {
        snapshot: PublicReadinessSnapshot,
    }

    impl PublicReadinessProbe for StaticSnapshotProbe {
        fn snapshot(&self) -> PublicReadinessSnapshot {
            self.snapshot.clone()
        }
    }

    const ADMIN_KEY: &str = "sk_live_admin_secret";
    const OTHER_ADMIN_KEY: &str = "sk_live_other_admin_secret";
    const INGEST_KEY: &str = "sk_live_ingest_secret";
    const OTHER_INGEST_KEY: &str = "sk_live_other_ingest_secret";
    const WRONG_INGEST_PREFIX_KEY: &str = "sk_live_ingest_wrong";
    const READ_KEY: &str = "sk_live_read_secret";
    const OTHER_READ_KEY: &str = "sk_live_other_read_secret";
    const RESPONDER_KEY: &str = "sk_live_responder_secret";
    const REVOKED_KEY: &str = "sk_live_revoked_secret";
    static ADMIN_ACCEPTED_KEYS: &[&str] = &[ADMIN_KEY];
    static INGEST_ACCEPTED_KEYS: &[&str] = &[ADMIN_KEY, INGEST_KEY];
    static READ_ACCEPTED_KEYS: &[&str] = &[ADMIN_KEY, READ_KEY, RESPONDER_KEY];
    static RESPONDER_WRITE_ACCEPTED_KEYS: &[&str] = &[ADMIN_KEY, RESPONDER_KEY];
    static ADMIN_REJECTED_KEYS: &[&str] = &[INGEST_KEY, READ_KEY, RESPONDER_KEY];
    static INGEST_REJECTED_KEYS: &[&str] = &[READ_KEY, RESPONDER_KEY];
    static READ_REJECTED_KEYS: &[&str] = &[INGEST_KEY];
    static RESPONDER_WRITE_REJECTED_KEYS: &[&str] = &[INGEST_KEY, READ_KEY];
    const TEST_BCRYPT_COST: u32 = 4;
    static TEMP_DB_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[derive(Clone, Copy)]
    struct AuthenticatedRouteSpec {
        method: &'static str,
        openapi_path: &'static str,
        sample_path: &'static str,
        required_scope: &'static str,
    }

    const fn auth_route(
        method: &'static str,
        openapi_path: &'static str,
        sample_path: &'static str,
        required_scope: &'static str,
    ) -> AuthenticatedRouteSpec {
        AuthenticatedRouteSpec {
            method,
            openapi_path,
            sample_path,
            required_scope,
        }
    }

    static AUTHENTICATED_ROUTE_SPECS: &[AuthenticatedRouteSpec] = &[
        auth_route("GET", "/metrics", "/metrics", "admin"),
        auth_route("POST", "/api/v1/errors", "/api/v1/errors", "ingest-only"),
        auth_route("POST", "/api/v1/events", "/api/v1/events", "ingest-only"),
        auth_route(
            "POST",
            "/api/v1/check-ins",
            "/api/v1/check-ins",
            "ingest-only",
        ),
        auth_route("GET", "/api/v1/query", "/api/v1/query", "read-only"),
        auth_route("GET", "/api/v1/report", "/api/v1/report", "read-only"),
        auth_route("GET", "/api/v1/timeline", "/api/v1/timeline", "read-only"),
        auth_route(
            "GET",
            "/api/v1/webhook-deliveries",
            "/api/v1/webhook-deliveries",
            "read-only",
        ),
        auth_route(
            "GET",
            "/api/v1/webhook-deliveries/{delivery_id}",
            "/api/v1/webhook-deliveries/DLV-route",
            "read-only",
        ),
        auth_route("GET", "/api/v1/status", "/api/v1/status", "read-only"),
        auth_route(
            "GET",
            "/api/v1/health-status",
            "/api/v1/health-status",
            "read-only",
        ),
        auth_route(
            "GET",
            "/api/v1/targets/{id}/checks",
            "/api/v1/targets/TGT-route/checks",
            "read-only",
        ),
        auth_route("GET", "/api/v1/incidents", "/api/v1/incidents", "read-only"),
        auth_route(
            "GET",
            "/api/v1/incidents/{id}",
            "/api/v1/incidents/INC-route",
            "read-only",
        ),
        auth_route(
            "POST",
            "/api/v1/incidents/{id}/escalate",
            "/api/v1/incidents/INC-route/escalate",
            "responder-write",
        ),
        auth_route(
            "POST",
            "/api/v1/incidents/{id}/deescalate",
            "/api/v1/incidents/INC-route/deescalate",
            "responder-write",
        ),
        auth_route(
            "GET",
            "/api/v1/incidents/{incident_id}/annotations",
            "/api/v1/incidents/INC-route/annotations",
            "read-only",
        ),
        auth_route(
            "POST",
            "/api/v1/incidents/{incident_id}/annotations",
            "/api/v1/incidents/INC-route/annotations",
            "responder-write",
        ),
        auth_route(
            "GET",
            "/api/v1/groups/{group_hash}/annotations",
            "/api/v1/groups/group-route/annotations",
            "read-only",
        ),
        auth_route(
            "POST",
            "/api/v1/groups/{group_hash}/annotations",
            "/api/v1/groups/group-route/annotations",
            "responder-write",
        ),
        auth_route(
            "GET",
            "/api/v1/annotations",
            "/api/v1/annotations",
            "read-only",
        ),
        auth_route(
            "POST",
            "/api/v1/annotations",
            "/api/v1/annotations",
            "responder-write",
        ),
        auth_route("GET", "/api/v1/claims", "/api/v1/claims", "read-only"),
        auth_route(
            "POST",
            "/api/v1/claims",
            "/api/v1/claims",
            "responder-write",
        ),
        auth_route(
            "GET",
            "/api/v1/claims/{id}",
            "/api/v1/claims/CLM-route",
            "read-only",
        ),
        auth_route(
            "POST",
            "/api/v1/claims/{id}/transition",
            "/api/v1/claims/CLM-route/transition",
            "responder-write",
        ),
        auth_route(
            "POST",
            "/api/v1/claims/{id}/release",
            "/api/v1/claims/CLM-route/release",
            "responder-write",
        ),
        auth_route(
            "GET",
            "/api/v1/errors/{id}",
            "/api/v1/errors/ERR-route",
            "read-only",
        ),
        auth_route("GET", "/api/v1/monitors", "/api/v1/monitors", "admin"),
        auth_route("POST", "/api/v1/monitors", "/api/v1/monitors", "admin"),
        auth_route(
            "DELETE",
            "/api/v1/monitors/{id}",
            "/api/v1/monitors/MON-route",
            "admin",
        ),
        auth_route("GET", "/api/v1/webhooks", "/api/v1/webhooks", "admin"),
        auth_route("POST", "/api/v1/webhooks", "/api/v1/webhooks", "admin"),
        auth_route(
            "DELETE",
            "/api/v1/webhooks/{id}",
            "/api/v1/webhooks/WHK-route",
            "admin",
        ),
        auth_route(
            "POST",
            "/api/v1/webhooks/{id}/test",
            "/api/v1/webhooks/WHK-route/test",
            "admin",
        ),
        auth_route("GET", "/api/v1/keys", "/api/v1/keys", "admin"),
        auth_route("POST", "/api/v1/keys", "/api/v1/keys", "admin"),
        auth_route(
            "POST",
            "/api/v1/keys/{id}/revoke",
            "/api/v1/keys/KEY-route/revoke",
            "admin",
        ),
        auth_route(
            "POST",
            "/api/v1/service-onboarding",
            "/api/v1/service-onboarding",
            "admin",
        ),
        auth_route("GET", "/api/v1/targets", "/api/v1/targets", "admin"),
        auth_route("POST", "/api/v1/targets", "/api/v1/targets", "admin"),
        auth_route(
            "PATCH",
            "/api/v1/targets/{id}",
            "/api/v1/targets/TGT-route",
            "admin",
        ),
        auth_route(
            "DELETE",
            "/api/v1/targets/{id}",
            "/api/v1/targets/TGT-route",
            "admin",
        ),
        auth_route(
            "POST",
            "/api/v1/targets/{id}/pause",
            "/api/v1/targets/TGT-route/pause",
            "admin",
        ),
        auth_route(
            "POST",
            "/api/v1/targets/{id}/resume",
            "/api/v1/targets/TGT-route/resume",
            "admin",
        ),
    ];

    fn authenticated_route_specs() -> &'static [AuthenticatedRouteSpec] {
        AUTHENTICATED_ROUTE_SPECS
    }

    fn accepted_scope_keys(
        required_scope: &str,
    ) -> Result<&'static [&'static str], Box<dyn Error>> {
        match required_scope {
            "admin" => Ok(ADMIN_ACCEPTED_KEYS),
            "ingest-only" => Ok(INGEST_ACCEPTED_KEYS),
            "read-only" => Ok(READ_ACCEPTED_KEYS),
            "responder-write" => Ok(RESPONDER_WRITE_ACCEPTED_KEYS),
            scope => Err(format!("unknown scope in route spec: {scope}").into()),
        }
    }

    fn rejected_scope_keys(
        required_scope: &str,
    ) -> Result<&'static [&'static str], Box<dyn Error>> {
        match required_scope {
            "admin" => Ok(ADMIN_REJECTED_KEYS),
            "ingest-only" => Ok(INGEST_REJECTED_KEYS),
            "read-only" => Ok(READ_REJECTED_KEYS),
            "responder-write" => Ok(RESPONDER_WRITE_REJECTED_KEYS),
            scope => Err(format!("unknown scope in route spec: {scope}").into()),
        }
    }

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
                    "supervisor": "ok",
                    "workers": []
                }
            })
        );

        Ok(())
    }

    #[tokio::test]
    async fn readyz_returns_worker_lifecycle_snapshot() -> Result<(), Box<dyn Error>> {
        let readiness = PublicReadiness::from_probe(Arc::new(StaticSnapshotProbe {
            snapshot: PublicReadinessSnapshot::with_workers(
                DependencyStatus::Ok,
                DependencyStatus::Ok,
                vec![
                    WorkerReadyzCheck {
                        name: "webhook_delivery".to_owned(),
                        state: WorkerLifecycleState::Started,
                        health: WorkerHealthStatus::Ok,
                        last_success_at: Some("2026-06-12T20:00:00Z".to_owned()),
                        last_success_age_ms: Some(500),
                        failure_count: 0,
                        consecutive_failures: 0,
                        last_error_class: None,
                        pressure_shape: WorkerPressureShape::Queue,
                        due_count: 0,
                        in_flight_count: 0,
                        oldest_due_age_ms: None,
                        oldest_due_item: None,
                        backoff_or_circuit_open: false,
                    },
                    WorkerReadyzCheck {
                        name: "target_probe".to_owned(),
                        state: WorkerLifecycleState::Stopped,
                        health: WorkerHealthStatus::Stopped,
                        last_success_at: None,
                        last_success_age_ms: None,
                        failure_count: 1,
                        consecutive_failures: 1,
                        last_error_class: Some("panic".to_owned()),
                        pressure_shape: WorkerPressureShape::Queue,
                        due_count: 0,
                        in_flight_count: 0,
                        oldest_due_age_ms: None,
                        oldest_due_item: None,
                        backoff_or_circuit_open: false,
                    },
                ],
            ),
        }));

        let response = public_router(readiness)
            .oneshot(Request::get("/readyz").body(Body::empty())?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "not_ready");
        assert_eq!(body["checks"]["workers"][0]["name"], "webhook_delivery");
        assert_eq!(body["checks"]["workers"][0]["state"], "started");
        assert_eq!(
            body["checks"]["workers"][0]["last_success_at"],
            "2026-06-12T20:00:00Z"
        );
        assert_eq!(body["checks"]["workers"][1]["name"], "target_probe");
        assert_eq!(body["checks"]["workers"][1]["state"], "stopped");
        assert_eq!(body["checks"]["workers"][1]["failure_count"], 1);
        assert_eq!(body["checks"]["workers"][1]["last_error_class"], "panic");

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
    async fn readyz_reads_live_probe_on_each_request() -> Result<(), Box<dyn Error>> {
        let probe = Arc::new(ToggleReadinessProbe {
            ready: AtomicBool::new(true),
        });
        let router = public_router(PublicReadiness::from_probe(probe.clone()));

        let ready_response = router
            .clone()
            .oneshot(Request::get("/readyz").body(Body::empty())?)
            .await?;
        assert_eq!(ready_response.status(), StatusCode::OK);

        probe.ready.store(false, Ordering::SeqCst);
        let not_ready_response = router
            .oneshot(Request::get("/readyz").body(Body::empty())?)
            .await?;
        assert_eq!(not_ready_response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json_body(not_ready_response).await?["status"], "not_ready");

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

        let dashboard = server
            .router()
            .oneshot(Request::get("/ui").body(Body::empty())?)
            .await?;
        assert_eq!(dashboard.status(), StatusCode::OK);
        assert_eq!(
            dashboard.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/html; charset=utf-8"))
        );

        let query = server
            .router()
            .oneshot(read_request(READ_KEY, "/api/v1/query?service=test-svc")?)
            .await?;
        assert_eq!(
            query.status(),
            StatusCode::UNAUTHORIZED,
            "boot should seed only the one-time bootstrap admin key"
        );
        assert!(server.enqueue_failure_snapshot().is_empty());
        assert_eq!(server.retention_prune_failure_count(), 0);
        assert_eq!(server.tls_expiry_scan_failure_count(), 0);
        let started = Instant::now();
        let workers = loop {
            let workers = server.worker_health_snapshot();
            if workers.len() == 5
                && workers.iter().all(|worker| {
                    worker.state == WorkerLifecycleState::Started
                        && worker.last_success_at.is_some()
                        && worker.failure_count == 0
                })
            {
                break workers;
            }
            if started.elapsed() > StdDuration::from_secs(1) {
                return Err(format!("timed out waiting for worker health: {workers:?}").into());
            }
            thread::sleep(StdDuration::from_millis(10));
        };
        assert_eq!(
            workers
                .iter()
                .map(|worker| worker.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "webhook_delivery",
                "target_probe",
                "monitor_overdue",
                "retention_prune",
                "tls_scan"
            ]
        );

        let ready = server
            .router()
            .oneshot(Request::get("/readyz").body(Body::empty())?)
            .await?;
        let ready_status = ready.status();
        let ready_body = json_body(ready).await?;
        assert_eq!(ready_status, StatusCode::OK);
        assert_eq!(
            ready_body["checks"]["workers"].as_array().map(Vec::len),
            Some(5)
        );
        assert_eq!(
            ready_body["checks"]["workers"][0]["name"],
            "webhook_delivery"
        );
        assert_eq!(ready_body["checks"]["workers"][0]["state"], "started");

        drop_server(server).await?;
        let store = {
            let mut store = Store::open(&path)?;
            store.migrate()?;
            store
        };
        let keys = store.list_api_keys()?;
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].name, "bootstrap");
        assert_eq!(keys[0].scope, "admin");
        fs::remove_file(path)?;

        Ok(())
    }

    /// Read routes must return identical results whether or not a
    /// `ReadPool` is wired: booting with one (the production path) reads
    /// through read-only connections instead of the writer, and the
    /// response must match the pre-existing writer-only behavior exactly.
    #[tokio::test]
    async fn read_pool_serves_report_route_with_matching_data() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("read-pool-parity");
        let mut store = Store::open(&path)?;
        store.migrate()?;
        seed_api_key(&mut store, "KEY-read", READ_KEY, "read-only", None)?;
        seed_api_key(&mut store, "KEY-ingest", INGEST_KEY, "ingest-only", None)?;
        let read_pool = Arc::new(ReadPool::open(&path)?);
        let state =
            IngestState::new(store, IngestConfig::default()).with_read_pool(read_pool.clone());
        let router = ingest_router(state.clone());

        let ingested = router
            .clone()
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        assert_eq!(ingested.status(), StatusCode::CREATED);

        let pooled = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/report?window=1h")?)
            .await?;
        assert_eq!(pooled.status(), StatusCode::OK);
        let pooled_body = json_body(pooled).await?;

        let writer_only_state = IngestState::new_with_shared_effect_sink(
            state.shared_store(),
            IngestConfig::default(),
            Arc::new(TestNoopIngestEffectSink),
        );
        let writer_only_router = ingest_router(writer_only_state);
        let writer_only = writer_only_router
            .oneshot(read_request(READ_KEY, "/api/v1/report?window=1h")?)
            .await?;
        assert_eq!(writer_only.status(), StatusCode::OK);
        let writer_only_body = json_body(writer_only).await?;

        assert_eq!(
            pooled_body["error_groups"],
            writer_only_body["error_groups"]
        );
        assert_eq!(pooled_body["summary"], writer_only_body["summary"]);
        assert_eq!(
            pooled_body["error_groups"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1
        );

        fs::remove_file(&path)?;
        let _ = fs::remove_file(format!("{}-wal", path.display()));
        let _ = fs::remove_file(format!("{}-shm", path.display()));

        Ok(())
    }

    /// `report_error_groups_scoped` and `active_incidents` deliberately stay
    /// off the read pool because they fuse a claim-expiry write into the
    /// read (see `read_pool.rs` and `read_source.rs`). This guards that the
    /// exclusion actually keeps a pooled `/api/v1/report` read correct end
    /// to end: an expired claim must not still show as `current_claim`
    /// (canary-930 child B review MINOR).
    #[tokio::test]
    async fn read_pool_report_reflects_claim_expiry_done_via_writer() -> Result<(), Box<dyn Error>>
    {
        let path = temp_db_path("read-pool-claim-expiry");
        let mut store = Store::open(&path)?;
        store.migrate()?;
        seed_api_key(&mut store, "KEY-admin", ADMIN_KEY, "admin", None)?;
        seed_api_key(&mut store, "KEY-read", READ_KEY, "read-only", None)?;
        seed_api_key(&mut store, "KEY-ingest", INGEST_KEY, "ingest-only", None)?;
        let read_pool = Arc::new(ReadPool::open(&path)?);
        let state = IngestState::new(store, IngestConfig::default()).with_read_pool(read_pool);
        let router = ingest_router(state);

        let ingested = router
            .clone()
            .oneshot(error_request(
                INGEST_KEY,
                r#"{"service":"claim-svc","error_class":"RuntimeError","message":"claim expiry regression"}"#,
            )?)
            .await?;
        assert_eq!(ingested.status(), StatusCode::CREATED);
        let ingested_body = json_body(ingested).await?;
        let group_hash = ingested_body["group_hash"]
            .as_str()
            .ok_or("missing error group hash")?
            .to_owned();

        let claimed = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/claims",
                ADMIN_KEY,
                &format!(
                    r#"{{"subject_type":"error_group","subject_id":"{group_hash}","owner":"codex","purpose":"inspect","ttl_ms":1,"idempotency_key":"run-expiry"}}"#
                ),
            )?)
            .await?;
        assert_eq!(claimed.status(), StatusCode::CREATED);

        // Let the 1ms TTL lapse so the claim is due for expiry by the time
        // the report handler's writer block runs it.
        thread::sleep(StdDuration::from_millis(50));

        let report = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/report?window=30d")?)
            .await?;
        assert_eq!(report.status(), StatusCode::OK);
        let report_body = json_body(report).await?;
        assert_eq!(report_body["error_groups"][0]["group_hash"], group_hash);
        assert!(
            report_body["error_groups"][0]["current_claim"].is_null(),
            "expired claim must not still be reported as current: {report_body}"
        );

        fs::remove_file(&path)?;
        let _ = fs::remove_file(format!("{}-wal", path.display()));
        let _ = fs::remove_file(format!("{}-shm", path.display()));

        Ok(())
    }

    #[tokio::test]
    async fn dashboard_shell_serves_assets_without_private_data() -> Result<(), Box<dyn Error>> {
        let router = dashboard_router();

        let shell = router
            .clone()
            .oneshot(Request::get("/ui").body(Body::empty())?)
            .await?;
        assert_eq!(shell.status(), StatusCode::OK);
        assert_eq!(
            shell.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/html; charset=utf-8"))
        );
        assert_eq!(
            shell.headers().get(CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-store"))
        );
        let shell_body = text_body(shell).await?;
        assert!(shell_body.contains("data-canary-dashboard"));
        assert!(!shell_body.contains("sk_live_"));
        assert!(!shell_body.contains("/api/v1/status"));

        let script = router
            .clone()
            .oneshot(Request::get("/ui/app.js").body(Body::empty())?)
            .await?;
        assert_eq!(script.status(), StatusCode::OK);
        assert_eq!(
            script.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/javascript; charset=utf-8"))
        );
        let script_body = text_body(script).await?;
        assert!(script_body.contains("Authorization"));
        assert!(script_body.contains("localStorage"));
        assert!(!script_body.contains("key: sessionStorage.getItem(KEY_STORAGE)"));
        assert!(script_body.contains("renderAuthChrome"));
        assert!(script_body.contains("activeIncidentFeed"));
        assert!(!script_body.contains("source: \"history\""));

        let aesthetic = router
            .oneshot(Request::get("/ui/aesthetic.css").body(Body::empty())?)
            .await?;
        assert_eq!(aesthetic.status(), StatusCode::OK);
        let aesthetic_body = text_body(aesthetic).await?;
        assert!(aesthetic_body.contains("aesthetic v2.16.0"));

        Ok(())
    }

    #[tokio::test]
    async fn canary_server_readyz_reports_stopped_worker() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("readyz-stopped-worker");
        let config = ServerConfig {
            webhook_drain_interval: StdDuration::from_secs(60),
            ..ServerConfig::new(path.clone())
        };
        let server = CanaryServer::boot(config)?;
        let router = server.router();

        let _ready = router
            .clone()
            .oneshot(Request::get("/readyz").body(Body::empty())?)
            .await?;

        server.stop_webhook_delivery_worker_for_test();
        let started = Instant::now();
        loop {
            let workers = server.worker_health_snapshot();
            if workers.iter().any(|worker| {
                worker.name == "webhook_delivery" && worker.state == WorkerLifecycleState::Stopped
            }) {
                break;
            }
            if started.elapsed() > StdDuration::from_secs(1) {
                drop_server(server).await?;
                return Err(format!("timed out waiting for stopped worker: {workers:?}").into());
            }
            thread::sleep(StdDuration::from_millis(10));
        }

        let not_ready = router
            .oneshot(Request::get("/readyz").body(Body::empty())?)
            .await?;
        let not_ready_status = not_ready.status();
        let not_ready_body = json_body(not_ready).await?;

        assert_eq!(not_ready_status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(not_ready_body["status"], "not_ready");
        assert_eq!(
            not_ready_body["checks"]["workers"][0]["name"],
            "webhook_delivery"
        );
        assert_eq!(not_ready_body["checks"]["workers"][0]["state"], "stopped");

        drop_server(server).await?;
        fs::remove_file(path)?;

        Ok(())
    }

    #[test]
    fn canary_server_boot_fails_before_readyz_when_store_cannot_open() -> Result<(), Box<dyn Error>>
    {
        let path = temp_db_path("missing-parent")
            .with_extension("missing-parent-dir")
            .join("canary.db");
        let config = ServerConfig::new(path);

        let error = match CanaryServer::boot(config) {
            Ok(_) => return Err("store boot failure should prevent serving readyz".into()),
            Err(error) => error,
        };
        assert!(
            matches!(error, ServerBootError::Store(_)),
            "expected store boot error, got {error:?}"
        );
        assert!(
            error
                .to_string()
                .starts_with("store boot failed: sqlite error:"),
            "{error}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn ingest_router_mounts_authenticated_route_matrix() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);
        for spec in authenticated_route_specs() {
            let method = Method::from_bytes(spec.method.as_bytes())?;
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method.clone())
                        .uri(spec.sample_path)
                        .body(Body::empty())?,
                )
                .await?;

            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{} {}",
                spec.method,
                spec.sample_path
            );
            let body = json_body(response).await?;
            assert_eq!(
                body["code"], "invalid_api_key",
                "{} {}",
                spec.method, spec.sample_path
            );

            for accepted_scope_key in accepted_scope_keys(spec.required_scope)? {
                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method.clone())
                            .uri(spec.sample_path)
                            .header("authorization", format!("Bearer {accepted_scope_key}"))
                            .body(Body::empty())?,
                    )
                    .await?;

                assert!(
                    !matches!(
                        response.status(),
                        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                    ),
                    "{} {} should accept {} for {}, got {}",
                    spec.method,
                    spec.sample_path,
                    accepted_scope_key,
                    spec.required_scope,
                    response.status()
                );
            }

            for wrong_scope_key in rejected_scope_keys(spec.required_scope)? {
                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method.clone())
                            .uri(spec.sample_path)
                            .header("authorization", format!("Bearer {wrong_scope_key}"))
                            .body(Body::empty())?,
                    )
                    .await?;

                assert_eq!(
                    response.status(),
                    StatusCode::FORBIDDEN,
                    "{} {} should reject {} for {}",
                    spec.method,
                    spec.sample_path,
                    wrong_scope_key,
                    spec.required_scope
                );
                let body = json_body(response).await?;
                assert_eq!(
                    body["code"], "insufficient_scope",
                    "{} {}",
                    spec.method, spec.sample_path
                );
            }
        }

        Ok(())
    }

    /// Test-only handler proving a panic while holding the writer lock
    /// neither poisons subsequent `lock_store()` calls nor produces more
    /// than one failed response (canary-930).
    #[allow(clippy::expect_used, clippy::panic)]
    async fn test_panic_handler(
        axum::extract::State(state): axum::extract::State<IngestState>,
    ) -> StatusCode {
        let _guard = state
            .lock_store()
            .expect("lock_store is infallible under parking_lot");
        panic!("test_panic_handler: intentional panic to prove request-path panic containment");
    }

    /// `ingest_router` plus one test-only panicking route, merged in with
    /// the same [`CatchPanicLayer`] production applies so the panic path
    /// under test is the real one, not a stand-in.
    fn router_with_test_panic_route(state: IngestState) -> Router {
        let panic_router = Router::<IngestState>::new()
            .route("/api/v1/__test/panic", get(test_panic_handler))
            .with_state(state.clone())
            .layer(CatchPanicLayer::custom(handle_request_panic));
        ingest_router(state).merge(panic_router)
    }

    #[tokio::test]
    async fn request_handler_panic_yields_one_500_and_does_not_poison_the_writer_lock()
    -> Result<(), Box<dyn Error>> {
        let router = router_with_test_panic_route(test_ingest_state()?);

        let panicked = router
            .clone()
            .oneshot(
                Request::get("/api/v1/__test/panic")
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(panicked.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let panic_body = json_body(panicked).await?;
        assert_eq!(panic_body["code"], "internal_error");

        // The writer mutex must not be poisoned: the next authenticated
        // request against the real store must succeed, not fail closed
        // (canary-930: "request path must not poison the writer mutex").
        let next = router
            .oneshot(
                Request::get("/metrics")
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(next.status(), StatusCode::OK);

        Ok(())
    }

    #[tokio::test]
    async fn problem_details_responses_keep_shared_wire_contract() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        let missing_auth = router
            .clone()
            .oneshot(Request::get("/api/v1/query").body(Body::empty())?)
            .await?;
        assert_problem_details(missing_auth, StatusCode::UNAUTHORIZED, "invalid_api_key").await?;

        let wrong_scope = router
            .clone()
            .oneshot(
                Request::post("/api/v1/errors")
                    .header("authorization", format!("Bearer {READ_KEY}"))
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from(valid_error_body()))?,
            )
            .await?;
        assert_problem_details(wrong_scope, StatusCode::FORBIDDEN, "insufficient_scope").await?;

        let too_large = router
            .clone()
            .oneshot(
                Request::post("/api/v1/errors")
                    .header("authorization", format!("Bearer {INGEST_KEY}"))
                    .header("content-length", (MAX_JSON_BODY_BYTES + 1).to_string())
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::empty())?,
            )
            .await?;
        assert_problem_details(
            too_large,
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
        )
        .await?;

        let validation = router
            .oneshot(
                Request::post("/api/v1/monitors")
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from("{}"))?,
            )
            .await?;
        assert_problem_details(
            validation,
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_error",
        )
        .await?;

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
            store.insert_webhook_subscription(webhook_subscription_insert(
                "WHK-boot",
                &url,
                vec!["error.new_class".to_owned()],
                "test-webhook-secret",
                true,
                "2026-05-28T20:00:00Z",
            ))?;
        }

        let config = ServerConfig {
            webhook_drain_interval: StdDuration::from_millis(10),
            webhook_transport_builder: local_webhook_transport_builder,
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
    async fn error_ingest_enqueues_only_matching_service_scoped_webhooks()
    -> Result<(), Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        seed_api_key(&mut store, "KEY-ingest", INGEST_KEY, "ingest-only", None)?;
        let mut matching = webhook_subscription_insert(
            "WHK-test-svc",
            "https://example.test/test-svc",
            vec!["error.new_class".to_owned()],
            "test-webhook-secret",
            true,
            "2026-05-28T20:00:00Z",
        );
        matching.service = Some("test-svc".to_owned());
        store.insert_webhook_subscription(matching)?;
        let mut other = webhook_subscription_insert(
            "WHK-other-svc",
            "https://example.test/other-svc",
            vec!["error.new_class".to_owned()],
            "test-webhook-secret",
            true,
            "2026-05-28T20:00:01Z",
        );
        other.service = Some("other-svc".to_owned());
        store.insert_webhook_subscription(other)?;

        let scheduler = Arc::new(RecordingScheduler::default());
        let state = IngestState::new_with_webhook_scheduler(
            store,
            IngestConfig::default(),
            scheduler.clone(),
        );
        let response = ingest_router(state)
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);
        let jobs = scheduler.jobs()?;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].webhook_id, "WHK-test-svc");
        assert_eq!(
            jobs[0].payload["tenant_id"],
            canary_store::BOOTSTRAP_TENANT_ID
        );
        assert_eq!(
            jobs[0].payload["project_id"],
            canary_store::BOOTSTRAP_PROJECT_ID
        );
        assert_eq!(jobs[0].payload["error"]["service"], "test-svc");

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
            store.insert_webhook_subscription(webhook_subscription_insert(
                "WHK-incident",
                &url,
                vec!["incident.opened".to_owned()],
                "test-webhook-secret",
                true,
                "2026-05-28T20:00:00Z",
            ))?;
        }

        let config = ServerConfig {
            webhook_drain_interval: StdDuration::from_millis(10),
            webhook_transport_builder: local_webhook_transport_builder,
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
    async fn canary_server_boot_wires_retention_prune_worker() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("retention-prune");
        let now = server_time::current_utc();
        let old_created_at = server_time::format_rfc3339(now - time::Duration::days(31));
        let recent_created_at = server_time::format_rfc3339(now - time::Duration::days(1));
        {
            let mut store = Store::open(&path)?;
            store.migrate()?;
            for index in 0..1005 {
                store.commit_error_ingest(test_error_ingest(index, &old_created_at))?;
            }
            store.commit_error_ingest(test_error_ingest(2000, &recent_created_at))?;
        }

        let config = ServerConfig {
            retention_prune_interval: StdDuration::from_millis(10),
            ..ServerConfig::new(path.clone())
        };
        let server = CanaryServer::boot(config)?;
        wait_for_error_count(&path, 1)?;

        drop_server(server).await?;
        fs::remove_file(path)?;

        Ok(())
    }

    #[test]
    fn canary_server_boot_rejects_zero_webhook_drain_max_jobs() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("webhook-drain-max-zero");
        let config = ServerConfig {
            webhook_drain_max_jobs: 0,
            ..ServerConfig::new(path)
        };

        let error = match CanaryServer::boot(config) {
            Ok(_) => return Err("zero webhook drain max jobs should be rejected".into()),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "webhook drain max jobs must be greater than zero"
        );

        Ok(())
    }

    #[test]
    fn canary_server_boot_rejects_zero_target_probe_interval() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("target-probe-zero");
        let config = ServerConfig {
            target_probe_interval: StdDuration::ZERO,
            ..ServerConfig::new(path)
        };

        let error = match CanaryServer::boot(config) {
            Ok(_) => return Err("zero target probe interval should be rejected".into()),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "target probe interval must be greater than zero"
        );

        Ok(())
    }

    #[test]
    fn canary_server_boot_rejects_zero_monitor_overdue_interval() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("monitor-overdue-zero");
        let config = ServerConfig {
            monitor_overdue_interval: StdDuration::ZERO,
            ..ServerConfig::new(path)
        };

        let error = match CanaryServer::boot(config) {
            Ok(_) => return Err("zero monitor overdue interval should be rejected".into()),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "monitor overdue interval must be greater than zero"
        );

        Ok(())
    }

    #[test]
    fn canary_server_boot_rejects_zero_retention_prune_interval() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("retention-zero");
        let config = ServerConfig {
            retention_prune_interval: StdDuration::ZERO,
            ..ServerConfig::new(path)
        };

        let error = match CanaryServer::boot(config) {
            Ok(_) => return Err("zero interval should be rejected".into()),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "retention prune interval must be greater than zero"
        );

        Ok(())
    }

    #[test]
    fn canary_server_boot_rejects_zero_tls_expiry_scan_interval() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("tls-expiry-zero");
        let config = ServerConfig {
            tls_expiry_scan_interval: StdDuration::ZERO,
            ..ServerConfig::new(path)
        };

        let error = match CanaryServer::boot(config) {
            Ok(_) => return Err("zero interval should be rejected".into()),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "tls expiry scan interval must be greater than zero"
        );

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

    #[test]
    fn openapi_authenticated_operations_match_route_scope_contract() -> Result<(), Box<dyn Error>> {
        let document: Value = serde_json::from_str(OPENAPI_JSON)?;
        let expected = authenticated_route_specs()
            .iter()
            .map(|spec| (spec.method.to_owned(), spec.openapi_path.to_owned()))
            .collect::<std::collections::BTreeSet<_>>();
        let documented = openapi_authenticated_operations(&document)?;

        assert_eq!(documented, expected);

        for spec in authenticated_route_specs() {
            let operation = openapi_operation(&document, spec.openapi_path, spec.method)?;
            assert_eq!(
                operation["x-canary-required-scope"], spec.required_scope,
                "{} {}",
                spec.method, spec.openapi_path
            );
        }

        Ok(())
    }

    #[test]
    fn openapi_authenticated_route_operations_match_ingest_router_literals()
    -> Result<(), Box<dyn Error>> {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub fn ingest_router")
            .ok_or("missing ingest_router source")?;
        let router_source = &source[start..];
        let end = router_source
            .find(".with_state(state)")
            .ok_or("missing ingest_router terminator")?;
        let mounted_operations = route_operations_from_source(&router_source[..end])?;
        let expected_operations = authenticated_route_specs()
            .iter()
            .map(|spec| (spec.method.to_owned(), spec.openapi_path.to_owned()))
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(mounted_operations, expected_operations);

        Ok(())
    }

    #[test]
    fn openapi_webhook_delivery_lookup_contract_is_agent_addressable() -> Result<(), Box<dyn Error>>
    {
        let document: Value = serde_json::from_str(OPENAPI_JSON)?;
        let guide = document
            .pointer("/info/x-agent-guide/webhook_contract")
            .and_then(Value::as_str)
            .ok_or("missing webhook_contract guide")?;
        assert!(guide.contains("GET /api/v1/webhook-deliveries/{delivery_id}"));
        assert!(guide.to_ascii_lowercase().contains("x-delivery-id"));
        assert!(guide.to_ascii_lowercase().contains("x-canary-signature"));
        assert!(guide.to_ascii_lowercase().contains("x-timestamp"));
        assert!(guide.to_ascii_lowercase().contains("x-webhook-id"));

        let operation =
            openapi_operation(&document, "/api/v1/webhook-deliveries/{delivery_id}", "GET")?;
        assert_eq!(operation["x-canary-required-scope"], "read-only");
        assert!(
            operation
                .get("parameters")
                .and_then(Value::as_array)
                .is_some_and(|parameters| parameters.iter().any(|parameter| {
                    parameter.get("name").and_then(Value::as_str) == Some("delivery_id")
                        && parameter.get("in").and_then(Value::as_str) == Some("path")
                        && parameter.get("required").and_then(Value::as_bool) == Some(true)
                })),
            "delivery lookup must require a delivery_id path parameter"
        );
        let schema = operation
            .pointer("/responses/200/content/application~1json/schema")
            .ok_or("missing delivery lookup 200 JSON schema")?;
        assert_eq!(schema_ref_name(schema), Some("WebhookDelivery"));

        Ok(())
    }

    #[test]
    fn openapi_json_responses_have_summaries_or_documented_exceptions() -> Result<(), Box<dyn Error>>
    {
        let document: Value = serde_json::from_str(OPENAPI_JSON)?;
        let exceptions = openapi_summary_exceptions(&document)?;
        let mut missing = Vec::new();

        for (method, path, operation) in openapi_operations(&document)? {
            for (status, response) in operation["responses"]
                .as_object()
                .ok_or("operation responses must be an object")?
            {
                let Some(schema) = json_response_schema(&document, response)? else {
                    continue;
                };
                if schema_has_summary(&document, schema, &mut Vec::new()) {
                    continue;
                }
                if exceptions
                    .operations
                    .contains(&(method.clone(), path.clone()))
                {
                    continue;
                }
                let Some(schema_name) = schema_ref_name(schema) else {
                    missing.push(format!("{method} {path} {status}: inline schema"));
                    continue;
                };
                if !exceptions.schemas.contains(schema_name) {
                    missing.push(format!("{method} {path} {status}: {schema_name}"));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "missing deterministic summary or summary exception:\n{}",
            missing.join("\n")
        );

        Ok(())
    }

    #[test]
    fn openapi_remediation_claim_schemas_are_agent_validatable() -> Result<(), Box<dyn Error>> {
        let document: Value = serde_json::from_str(OPENAPI_JSON)?;
        let claim = document
            .pointer("/components/schemas/RemediationClaim")
            .ok_or("missing RemediationClaim schema")?;
        assert!(
            claim.get("allOf").is_none(),
            "RemediationClaim must be a single closed object, not closed allOf composition"
        );
        assert_eq!(claim["additionalProperties"], false);
        for field in [
            "id",
            "tenant_id",
            "project_id",
            "service",
            "subject_type",
            "subject_id",
            "owner",
            "purpose",
            "state",
            "idempotency_key",
            "evidence_links",
            "created_at",
            "updated_at",
            "expires_at",
            "released_at",
            "completed_at",
        ] {
            assert!(
                schema_required_field(claim, field),
                "RemediationClaim must require {field}"
            );
            assert!(
                claim.pointer(&format!("/properties/{field}")).is_some(),
                "RemediationClaim must define {field}"
            );
        }

        let conflict = document
            .pointer("/components/schemas/RemediationClaimConflictProblem")
            .ok_or("missing RemediationClaimConflictProblem schema")?;
        assert!(
            conflict.get("allOf").is_none(),
            "RemediationClaimConflictProblem must be a single closed object"
        );
        for field in [
            "type",
            "title",
            "status",
            "detail",
            "code",
            "request_id",
            "current_claim",
        ] {
            assert!(
                schema_required_field(conflict, field),
                "RemediationClaimConflictProblem must require {field}"
            );
            assert!(
                conflict.pointer(&format!("/properties/{field}")).is_some(),
                "RemediationClaimConflictProblem must define {field}"
            );
        }

        let response = document
            .pointer("/components/schemas/RemediationClaimsResponse")
            .ok_or("missing RemediationClaimsResponse schema")?;
        for field in ["limit", "cursor", "truncated"] {
            assert!(
                schema_required_field(response, field),
                "RemediationClaimsResponse must require {field}"
            );
            assert!(
                response.pointer(&format!("/properties/{field}")).is_some(),
                "RemediationClaimsResponse must define {field}"
            );
        }

        Ok(())
    }

    #[test]
    fn openapi_telemetry_events_are_contract_visible() -> Result<(), Box<dyn Error>> {
        let document: Value = serde_json::from_str(OPENAPI_JSON)?;
        let business_events = document
            .pointer("/components/schemas/BusinessEvent/enum")
            .and_then(Value::as_array)
            .ok_or("missing BusinessEvent enum")?;
        assert!(
            business_events
                .iter()
                .any(|event| event.as_str() == Some("telemetry.event")),
            "BusinessEvent must include telemetry.event"
        );
        let entity_types = document
            .pointer("/components/schemas/TimelineEvent/properties/entity_type/enum")
            .and_then(Value::as_array)
            .ok_or("missing TimelineEvent.entity_type enum")?;
        assert!(
            entity_types
                .iter()
                .any(|entity_type| entity_type.as_str() == Some("telemetry_event")),
            "TimelineEvent.entity_type must include telemetry_event"
        );

        Ok(())
    }

    #[test]
    fn openapi_agent_guide_covers_cold_start_and_write_back() -> Result<(), Box<dyn Error>> {
        let document: Value = serde_json::from_str(OPENAPI_JSON)?;
        let guide = document
            .pointer("/info/x-agent-guide")
            .and_then(Value::as_object)
            .ok_or("missing info.x-agent-guide")?;

        let cold_start = guide
            .get("cold_start")
            .and_then(Value::as_object)
            .ok_or("missing cold_start guide")?;
        for key in ["entrypoint", "pagination", "handoff"] {
            assert!(
                cold_start
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty()),
                "cold_start.{key} must be a non-empty string"
            );
        }
        let handoff = cold_start
            .get("handoff")
            .and_then(Value::as_str)
            .ok_or("missing cold_start.handoff")?;
        assert!(
            handoff.contains("Do not pass report.cursor"),
            "cold_start.handoff must reject report cursor reuse"
        );

        let annotation = guide
            .get("annotation_write_back")
            .and_then(Value::as_object)
            .ok_or("missing annotation_write_back guide")?;
        let actions = annotation
            .get("stable_actions")
            .and_then(Value::as_array)
            .ok_or("missing annotation stable_actions")?
            .iter()
            .filter_map(Value::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            actions,
            ["fix-proposed", "fix-verified", "noise-dismissed", "triaged"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
        assert!(
            annotation
                .get("metadata_keys")
                .and_then(Value::as_array)
                .is_some_and(|values| values.iter().all(Value::is_string) && !values.is_empty())
        );

        for schema_name in ["Annotation", "AnnotationRequest", "AnnotationCreateRequest"] {
            let action = document
                .pointer(&format!(
                    "/components/schemas/{schema_name}/properties/action"
                ))
                .and_then(Value::as_object)
                .ok_or_else(|| format!("missing {schema_name}.action"))?;
            let stable_values = action
                .get("x-canary-stable-values")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("missing {schema_name}.action stable values"))?
                .iter()
                .filter_map(Value::as_str)
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(stable_values, actions, "{schema_name}.action");

            let metadata = document
                .pointer(&format!(
                    "/components/schemas/{schema_name}/properties/metadata"
                ))
                .and_then(Value::as_object)
                .ok_or_else(|| format!("missing {schema_name}.metadata"))?;
            assert!(
                metadata
                    .get("x-canary-expected-keys")
                    .and_then(Value::as_array)
                    .is_some_and(|values| values.iter().all(Value::is_string) && !values.is_empty()),
                "{schema_name}.metadata must document expected keys"
            );
        }

        Ok(())
    }

    #[test]
    fn openapi_primary_agent_entrypoints_have_operation_guidance() -> Result<(), Box<dyn Error>> {
        let document: Value = serde_json::from_str(OPENAPI_JSON)?;
        for (method, path) in [
            ("GET", "/api/v1/report"),
            ("GET", "/api/v1/timeline"),
            ("GET", "/api/v1/incidents/{id}"),
            ("POST", "/api/v1/check-ins"),
            ("POST", "/api/v1/events"),
            ("POST", "/api/v1/incidents/{incident_id}/annotations"),
            ("POST", "/api/v1/groups/{group_hash}/annotations"),
            ("POST", "/api/v1/annotations"),
        ] {
            let operation = openapi_operation(&document, path, method)?;
            let guidance = operation
                .get("x-agent-guidance")
                .and_then(Value::as_object)
                .ok_or_else(|| format!("missing x-agent-guidance for {method} {path}"))?;
            for key in ["when_to_call", "trust", "next"] {
                assert!(
                    guidance
                        .get(key)
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.is_empty()),
                    "{method} {path} guidance.{key} must be a non-empty string"
                );
            }
        }

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
    async fn browser_ingest_routes_answer_cors_preflight() -> Result<(), Box<dyn Error>> {
        for path in ["/api/v1/errors", "/api/v1/events"] {
            let response = ingest_router(test_ingest_state()?)
                .oneshot(
                    Request::builder()
                        .method(Method::OPTIONS)
                        .uri(path)
                        .header("origin", "https://www.chrondle.app")
                        .header("access-control-request-method", "POST")
                        .header(
                            "access-control-request-headers",
                            "authorization,content-type",
                        )
                        .body(Body::empty())?,
                )
                .await?;

            assert!(response.status().is_success(), "{path}");
            let headers = response.headers();
            assert_eq!(
                headers.get("access-control-allow-origin"),
                Some(&HeaderValue::from_static("*")),
                "{path}"
            );
            let allow_methods = headers
                .get("access-control-allow-methods")
                .ok_or("missing allowed methods")?
                .to_str()?;
            assert!(allow_methods.contains("POST"), "{path}");
            let allow_headers = headers
                .get("access-control-allow-headers")
                .ok_or("missing allowed headers")?
                .to_str()?
                .to_ascii_lowercase();
            assert!(allow_headers.contains("authorization"), "{path}");
            assert!(allow_headers.contains("content-type"), "{path}");
        }

        Ok(())
    }

    #[tokio::test]
    async fn browser_ingest_post_includes_cors_response_header() -> Result<(), Box<dyn Error>> {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(
                Request::post("/api/v1/errors")
                    .header("authorization", format!("Bearer {INGEST_KEY}"))
                    .header("origin", "https://www.chrondle.app")
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from(valid_error_body()))?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.headers().get("access-control-allow-origin"),
            Some(&HeaderValue::from_static("*"))
        );

        Ok(())
    }

    #[tokio::test]
    async fn read_routes_do_not_gain_browser_cors() -> Result<(), Box<dyn Error>> {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(
                Request::get("/api/v1/query?service=test-svc")
                    .header("authorization", format!("Bearer {READ_KEY}"))
                    .header("origin", "https://www.chrondle.app")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none()
        );

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

        let store = state.lock_store().map_err(|_| "store lock poisoned")?;
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

        let store = state.lock_store().map_err(|_| "store lock poisoned")?;
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
        state.replace_effect_sink(Arc::new(WebhookEnqueueEffectSink::new(
            state.shared_store(),
            scheduler.clone(),
            cooldown,
        )));
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

        let store = state.lock_store().map_err(|_| "store lock poisoned")?;
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

        let store = store.lock();
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
        let store = store.lock();
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

        let store = store.lock();
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
        let store = store.lock();
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
                "WHK-http-ok",
                Some("2026-05-28T20:00:00Z".to_owned()),
                Some(42),
            ),
        };
        let transport = HttpWebhookTransport::with_timeout_allowing_private_destinations(
            StdDuration::from_secs(10),
        )?;

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
            header_value(&captured.head, "x-webhook-id").as_deref(),
            Some("WHK-http-ok")
        );
        assert_eq!(
            header_value(&captured.head, "x-timestamp").as_deref(),
            Some("2026-05-28T20:00:00Z")
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
        assert_eq!(
            header_value(&captured.head, "x-canary-signature").as_deref(),
            Some(request.headers.canary_signature.as_str())
        );
        assert!(
            canary_http::webhooks::verify_timestamped_signature(
                captured.body.as_bytes(),
                "test-webhook-secret",
                "2026-05-28T20:00:00Z",
                "DLV-http-ok",
                &request.headers.canary_signature,
            ),
            "receiver should be able to verify timestamp-bound Canary signature"
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
                "WHK-http-redirect",
                None,
                None,
            ),
        };
        let transport = HttpWebhookTransport::with_timeout_allowing_private_destinations(
            StdDuration::from_secs(10),
        )?;

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
                "WHK-http-503",
                None,
                None,
            ),
        };
        let transport = HttpWebhookTransport::with_timeout_allowing_private_destinations(
            StdDuration::from_secs(10),
        )?;

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
                "WHK-http-error",
                None,
                None,
            ),
        };
        let transport = HttpWebhookTransport::with_timeout_allowing_private_destinations(
            StdDuration::from_millis(200),
        )?;

        let TransportResult::RequestError(reason) = transport.send(&request) else {
            return Err("connection failure should map to request error".into());
        };
        assert!(!reason.is_empty());

        Ok(())
    }

    #[test]
    fn http_webhook_transport_rejects_private_destination_before_request()
    -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let request = WebhookRequest {
            url: format!("http://{addr}/hook"),
            body: "{}".to_owned(),
            headers: canary_http::webhooks::headers_for_body(
                "{}",
                "test-webhook-secret",
                "error.new_class",
                "DLV-http-private",
                "WHK-http-private",
                None,
                None,
            ),
        };
        let transport = HttpWebhookTransport::with_timeout(StdDuration::from_millis(200))?;

        let TransportResult::RequestError(reason) = transport.send(&request) else {
            return Err("private destination should be rejected before HTTP request".into());
        };
        assert!(reason.contains("non-global") || reason.contains("localhost"));
        listener.set_nonblocking(true)?;
        match listener.accept() {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Ok(_) => return Err("transport should not connect to rejected destination".into()),
            Err(error) => return Err(error.into()),
        }

        Ok(())
    }

    #[test]
    fn webhook_delivery_runtime_uses_http_transport_and_records_ledger()
    -> Result<(), Box<dyn Error>> {
        let (url, server) = spawn_webhook_server(204, &[])?;
        let store = runtime_store_with_url(true, &url)?;
        let runtime = WebhookDeliveryRuntime::new_without_circuit(
            store.clone(),
            Arc::new(
                HttpWebhookTransport::with_timeout_allowing_private_destinations(
                    StdDuration::from_secs(10),
                )?,
            ),
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
        let store = store.lock();
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

        let mut store = store.lock();
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

        assert_eq!(report.claimed, 1);
        assert_eq!(report.completed, 1);
        assert_eq!(report.retried, 0);
        assert_eq!(report.discarded, 0);
        assert_eq!(report.recovered, 0);
        assert_eq!(report.recovery_retried, 0);
        assert_eq!(report.recovery_discarded, 0);
        assert_eq!(report.due_count, 1);
        assert!(report.oldest_due_age_ms.is_some());
        assert_eq!(
            transport
                .requests
                .lock()
                .map_err(|_| "transport lock poisoned")?
                .len(),
            1
        );
        let mut store = store.lock();
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

        assert_webhook_drain_report(&report, 1, 0, 1, 0);
        assert_eq!(report.due_count, 1);
        assert_eq!(report.oldest_due_age_ms, Some(0));
        let store = store.lock();
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
    fn webhook_delivery_drain_reschedules_claimed_job_after_runtime_panic()
    -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let job_id = insert_due_webhook_job(&store, "DLV-drain-panic", 4)?;
        let transport = Arc::new(PanicOnceTransport::new());
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport);
        let drain = WebhookDeliveryDrain::new(store.clone(), runtime, 10);

        let report = drain.drain_due("2026-05-28T20:00:00Z")?;

        assert_webhook_drain_report(&report, 1, 0, 1, 0);
        assert_eq!(report.due_count, 1);
        assert_eq!(report.oldest_due_age_ms, Some(0));
        let store = store.lock();
        let job = store
            .webhook_delivery_job(job_id)?
            .ok_or("missing webhook delivery job")?;
        assert_eq!(job.state, WebhookDeliveryJobState::Scheduled);
        assert_eq!(job.scheduled_at, "2026-05-28T20:00:01Z");
        assert_eq!(job.attempt, 1);
        assert_eq!(job.args["delivery_id"], "DLV-drain-panic");

        Ok(())
    }

    #[test]
    fn webhook_delivery_drain_rejects_invalid_retry_clock_without_claiming_job()
    -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        let job_id = insert_due_webhook_job(&store, "DLV-drain-invalid-clock", 4)?;
        let transport = Arc::new(RecordingTransport::status(500));
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport);
        let drain = WebhookDeliveryDrain::new(store.clone(), runtime, 10);

        let error = match drain.drain_due("not-a-rfc3339") {
            Ok(_) => return Err("invalid drain timestamp should fail".into()),
            Err(error) => error,
        };

        assert!(error.contains("invalid drain timestamp"));
        let store = store.lock();
        let job = store
            .webhook_delivery_job(job_id)?
            .ok_or("missing webhook delivery job")?;
        assert_eq!(job.state, WebhookDeliveryJobState::Available);
        assert_eq!(job.attempt, 0);

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
        assert_webhook_drain_report(&second, 1, 0, 0, 1);
        assert_eq!(second.due_count, 1);
        assert_eq!(second.oldest_due_age_ms, Some(0));
        let store = store.lock();
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

        assert_eq!(report.claimed, 1);
        assert_eq!(report.completed, 1);
        assert_eq!(report.retried, 0);
        assert_eq!(report.discarded, 0);
        assert_eq!(report.recovered, 0);
        assert_eq!(report.recovery_retried, 0);
        assert_eq!(report.recovery_discarded, 0);
        assert_eq!(report.due_count, 1);
        assert_eq!(report.oldest_due_age_ms, Some(0));
        assert_eq!(report.circuit_open_suppressed, 1);
        assert_eq!(
            transport
                .requests
                .lock()
                .map_err(|_| "transport lock poisoned")?
                .len(),
            0
        );
        let store = store.lock();
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
        let mut store = store.lock();
        assert!(
            store
                .claim_due_webhook_delivery_jobs("9999-01-01T00:00:00Z", 10)?
                .is_empty()
        );

        Ok(())
    }

    #[test]
    fn webhook_delivery_drain_reports_due_backlog_pressure() -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        for index in 0..3 {
            insert_due_webhook_job(&store, &format!("DLV-pressure-{index}"), 4)?;
        }
        let transport = Arc::new(RecordingTransport::status(204));
        let runtime = WebhookDeliveryRuntime::new_without_circuit(store.clone(), transport);
        let drain = WebhookDeliveryDrain::new(store, runtime, 1);

        let report = drain.drain_due("2026-05-28T20:02:00Z")?;

        assert_webhook_drain_report(&report, 1, 1, 0, 0);
        assert_eq!(report.due_count, 3);
        assert_eq!(report.oldest_due_age_ms, Some(120_000));

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

    #[test]
    fn shared_store_survives_concurrent_runtime_pressure() -> Result<(), Box<dyn Error>> {
        let store = runtime_store(true)?;
        {
            let mut locked = store.lock();
            seed_target(&mut locked, "runtime-pressure")?;
            for index in 0..80 {
                locked.commit_error_ingest(test_error_ingest(index, "2026-04-01T00:00:00Z"))?;
            }
        }
        for index in 0..16 {
            insert_due_webhook_job(&store, &format!("DLV-pressure-{index}"), 4)?;
        }

        let ingest_store = store.clone();
        let ingest = thread::spawn(move || -> Result<(), String> {
            for index in 100..140 {
                let mut store = ingest_store.lock();
                store
                    .commit_error_ingest(test_error_ingest(index, "2026-05-28T20:00:00Z"))
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        });

        let webhook_store = store.clone();
        let webhooks = thread::spawn(move || -> Result<(), String> {
            for _ in 0..4 {
                let mut store = webhook_store.lock();
                let claimed = store
                    .claim_due_webhook_delivery_jobs("2026-05-28T20:00:10Z", 4)
                    .map_err(|error| error.to_string())?;
                for job in claimed {
                    let applied = store
                        .complete_webhook_delivery_job(
                            &job,
                            WebhookDeliveryJobCompletion::Retry {
                                scheduled_at: "2026-05-28T20:01:00Z".to_owned(),
                            },
                        )
                        .map_err(|error| error.to_string())?;
                    if !applied {
                        return Err(format!("webhook job {} execution lease lost", job.id));
                    }
                }
                let _ = store
                    .recover_stale_webhook_delivery_jobs(
                        "2026-05-28T20:02:00Z",
                        "2026-05-28T20:01:00Z",
                        16,
                    )
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        });

        let probe_store = store.clone();
        let probes = thread::spawn(move || -> Result<(), String> {
            for _ in 0..40 {
                let store = probe_store.lock();
                let schedules = store
                    .active_target_probe_schedules()
                    .map_err(|error| error.to_string())?;
                if schedules.len() != 1 {
                    return Err(format!(
                        "expected one active schedule, got {}",
                        schedules.len()
                    ));
                }
            }
            Ok(())
        });

        let retention = RetentionPruneLifecycle::new(
            store.clone(),
            RetentionPolicy {
                error_retention_days: 30,
                check_retention_days: 7,
            },
        );
        let pruning = thread::spawn(move || {
            retention.run_due(
                time::OffsetDateTime::parse(
                    "2026-05-29T12:00:00Z",
                    &time::format_description::well_known::Rfc3339,
                )
                .map_err(|error| error.to_string())?,
            )
        });

        for result in [ingest, webhooks, probes] {
            result
                .join()
                .map_err(|_| "runtime pressure lane panicked")??;
        }
        let prune_report = pruning
            .join()
            .map_err(|_| "retention pressure lane panicked")??;
        assert!(prune_report.batches >= 1);

        let store = store.lock();
        assert_eq!(store.active_target_probe_schedules()?.len(), 1);
        assert!(
            store.health_targets()?.iter().any(|target| {
                target.service == "runtime-pressure" && target.state == "unknown"
            })
        );

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
    async fn monitor_check_in_accepts_ingest_scope_and_returns_body() -> Result<(), Box<dyn Error>>
    {
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
    async fn monitor_check_in_uses_key_owner_to_resolve_monitor_name() -> Result<(), Box<dyn Error>>
    {
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_monitor(&mut store, "worker-heartbeat")?;
            seed_api_key_with_owner(
                &mut store,
                "KEY-other-ingest",
                OTHER_INGEST_KEY,
                "ingest-only",
                None,
                "TENANT-other",
                "PROJECT-other",
            )?;
            store.create_monitor_scoped(
                MonitorInsert {
                    id: "MON-other-worker".to_owned(),
                    name: "worker-heartbeat".to_owned(),
                    service: "other-worker".to_owned(),
                    mode: "ttl".to_owned(),
                    expected_every_ms: 90_000,
                    grace_ms: 5_000,
                    created_at: "2026-05-28T20:00:00Z".to_owned(),
                },
                "TENANT-other",
                "PROJECT-other",
            )?;
        }
        let router = ingest_router(state);

        let bootstrap = router
            .clone()
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"worker-heartbeat","status":"alive","observed_at":"2026-05-28T20:00:00Z"}"#,
            )?)
            .await?;
        assert_eq!(bootstrap.status(), StatusCode::CREATED);
        assert_eq!(
            json_body(bootstrap).await?["monitor_id"],
            "MON-worker-heartbeat"
        );

        let other = router
            .oneshot(check_in_request(
                OTHER_INGEST_KEY,
                r#"{"monitor":"worker-heartbeat","status":"alive","observed_at":"2026-05-28T20:01:00Z"}"#,
            )?)
            .await?;
        assert_eq!(other.status(), StatusCode::CREATED);
        assert_eq!(json_body(other).await?["monitor_id"], "MON-other-worker");

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
        let store = Arc::new(parking_lot::Mutex::new(store));
        let recorder = Arc::new(EnqueueFailureRecorder::default());
        let state = IngestState::new_with_shared_fanout(
            store,
            IngestConfig::default(),
            Arc::new(TestNoopIngestEffectSink),
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
    async fn monitor_check_in_returns_internal_problem_for_corrupt_persisted_monitor_mode()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            store.insert_monitor(MonitorInsert {
                id: "MON-corrupt-mode".to_owned(),
                name: "corrupt-mode".to_owned(),
                service: "corrupt-mode".to_owned(),
                mode: "weekly".to_owned(),
                expected_every_ms: 90_000,
                grace_ms: 5_000,
                created_at: "2026-05-28T20:00:00Z".to_owned(),
            })?;
        }

        let response = ingest_router(state)
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"corrupt-mode","status":"alive"}"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["code"], "internal_error");

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
    async fn monitor_check_in_rejects_future_observed_at_without_writing()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state_with_monitor("desktop-active-timer")?;
        let router = ingest_router(state.clone());

        let response = router
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"desktop-active-timer","status":"alive","observed_at":"2999-01-01T00:00:00Z","ttl_ms":120000}"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "validation_error");
        assert_eq!(body["detail"], "observed_at is too far in the future.");
        assert_eq!(
            body["errors"]["observed_at"],
            json!(["must not be more than 5 minutes in the future"])
        );

        let accepted = ingest_router(state)
            .oneshot(check_in_request(INGEST_KEY, valid_check_in_body())?)
            .await?;
        let accepted_body = json_body(accepted).await?;
        assert_eq!(accepted_body["sequence"], 1);

        Ok(())
    }

    #[tokio::test]
    async fn monitor_check_in_rejects_missing_invalid_revoked_and_wrong_scope_keys()
    -> Result<(), Box<dyn Error>> {
        let cases = [
            (
                Request::post("/api/v1/check-ins")
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from(valid_check_in_body()))?,
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
            ),
            (
                check_in_request("sk_live_unknown_secret", valid_check_in_body())?,
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
            ),
            (
                check_in_request(READ_KEY, valid_check_in_body())?,
                StatusCode::FORBIDDEN,
                "insufficient_scope",
            ),
            (
                check_in_request(REVOKED_KEY, valid_check_in_body())?,
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
            ),
        ];

        for (request, expected_status, expected_code) in cases {
            let response = ingest_router(test_ingest_state_with_monitor("desktop-active-timer")?)
                .oneshot(request)
                .await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, expected_status);
            assert_eq!(body["code"], expected_code);
        }

        Ok(())
    }

    #[tokio::test]
    async fn monitor_check_in_preflight_rejects_large_payload_without_writing()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state_with_monitor("desktop-active-timer")?;
        let router = ingest_router(state);

        let content_length_response = router
            .clone()
            .oneshot(
                Request::post("/api/v1/check-ins")
                    .header("authorization", format!("Bearer {INGEST_KEY}"))
                    .header("content-length", "102401")
                    .body(Body::from("{"))?,
            )
            .await?;
        let status = content_length_response.status();
        let body = json_body(content_length_response).await?;

        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body["code"], "payload_too_large");
        assert_eq!(body["detail"], "Request body exceeds 100KB limit.");

        let body_too_large = "x".repeat(102_401);
        let body_length_response = router
            .clone()
            .oneshot(
                Request::post("/api/v1/check-ins")
                    .header("authorization", format!("Bearer {INGEST_KEY}"))
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from(body_too_large))?,
            )
            .await?;
        let status = body_length_response.status();
        let body = json_body(body_length_response).await?;

        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body["code"], "payload_too_large");
        assert_eq!(body["detail"], "Request body exceeds 100KB limit.");

        let response = router
            .oneshot(check_in_request(INGEST_KEY, valid_check_in_body())?)
            .await?;
        let body = json_body(response).await?;
        assert_eq!(body["sequence"], 1);

        Ok(())
    }

    #[tokio::test]
    async fn monitor_check_in_decode_order_rejects_malformed_json_after_auth()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state_with_monitor("desktop-active-timer")?;
        let router = ingest_router(state);

        let malformed = router
            .clone()
            .oneshot(check_in_request(INGEST_KEY, "{")?)
            .await?;
        let status = malformed.status();
        let body = json_body(malformed).await?;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_request");

        let unauthorized = router
            .clone()
            .oneshot(Request::post("/api/v1/check-ins").body(Body::from("{"))?)
            .await?;
        let status = unauthorized.status();
        let body = json_body(unauthorized).await?;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "invalid_api_key");

        let response = router
            .oneshot(check_in_request(INGEST_KEY, valid_check_in_body())?)
            .await?;
        let body = json_body(response).await?;
        assert_eq!(body["sequence"], 1);

        Ok(())
    }

    #[tokio::test]
    async fn monitor_check_in_validation_failures_do_not_write() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state_with_monitor("desktop-active-timer")?;
        let router = ingest_router(state);

        let missing_status = router
            .clone()
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"desktop-active-timer"}"#,
            )?)
            .await?;
        assert_eq!(missing_status.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let invalid_observed_at = router
            .clone()
            .oneshot(check_in_request(
                INGEST_KEY,
                r#"{"monitor":"desktop-active-timer","status":"alive","observed_at":"not-a-time"}"#,
            )?)
            .await?;
        assert_eq!(
            invalid_observed_at.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let response = router
            .oneshot(check_in_request(INGEST_KEY, valid_check_in_body())?)
            .await?;
        let body = json_body(response).await?;
        assert_eq!(body["sequence"], 1);

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
        let snippet_keys = created["snippets"]
            .as_object()
            .ok_or("missing snippets object")?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            snippet_keys,
            vec![
                "error_ingest_curl",
                "report_curl",
                "service_query_curl",
                "typescript_init"
            ]
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
    async fn service_onboarding_inherits_admin_tenant_project_authority()
    -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?.with_target_control(recorder.clone());
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_api_key_with_owner(
                &mut store,
                "KEY-other-admin",
                OTHER_ADMIN_KEY,
                "admin",
                None,
                "TENANT-other",
                "PROJECT-other",
            )?;
        }
        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/service-onboarding",
                OTHER_ADMIN_KEY,
                r#"{
                    "service":"other-api",
                    "url":"https://example.com/other-api/health",
                    "environment":"production"
                }"#,
            )?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);
        let created = json_body(response).await?;
        let target_id = created["target"]["id"]
            .as_str()
            .ok_or("missing target id")?
            .to_owned();
        let key_id = created["api_key"]["id"]
            .as_str()
            .ok_or("missing key id")?
            .to_owned();

        let bootstrap_targets = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        assert_eq!(json_body(bootstrap_targets).await?["targets"], json!([]));

        let other_targets = router
            .clone()
            .oneshot(read_request(OTHER_ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        let other_targets = json_body(other_targets).await?;
        assert_eq!(other_targets["targets"][0]["id"], target_id);
        assert_eq!(other_targets["targets"][0]["service"], "other-api");

        let other_keys = router
            .oneshot(read_request(OTHER_ADMIN_KEY, "/api/v1/keys")?)
            .await?;
        let other_keys = json_body(other_keys).await?;
        let created_key = other_keys["keys"]
            .as_array()
            .and_then(|keys| keys.iter().find(|key| key["id"] == key_id))
            .ok_or("missing service key")?;
        assert_eq!(created_key["tenant_id"], "TENANT-other");
        assert_eq!(created_key["project_id"], "PROJECT-other");
        assert_eq!(created_key["service"], "other-api");

        assert_eq!(
            recorder.commands(),
            vec![TargetProbeLifecycleCommand::Track {
                target_id,
                interval_ms: 60_000,
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn service_onboarding_rejects_preflight_and_decode_failures_without_writes()
    -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?.with_target_control(recorder.clone());
        let router = ingest_router(state);

        let oversized = router
            .clone()
            .oneshot(
                Request::post("/api/v1/service-onboarding")
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .header("content-length", "102401")
                    .body(Body::from("{"))?,
            )
            .await?;
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(json_body(oversized).await?["code"], "payload_too_large");
        assert!(recorder.commands().is_empty());
        let targets = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        assert_eq!(json_body(targets).await?["targets"], json!([]));

        let malformed = router
            .clone()
            .oneshot(
                Request::post("/api/v1/service-onboarding")
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from("{"))?,
            )
            .await?;
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_body(malformed).await?["code"], "invalid_request");
        assert!(recorder.commands().is_empty());

        let unauthenticated = router
            .clone()
            .oneshot(
                Request::post("/api/v1/service-onboarding")
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from("{"))?,
            )
            .await?;
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(json_body(unauthenticated).await?["code"], "invalid_api_key");
        assert!(recorder.commands().is_empty());

        let targets = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        assert_eq!(json_body(targets).await?["targets"], json!([]));

        Ok(())
    }

    #[tokio::test]
    async fn service_onboarding_rejects_subsecond_target_interval_without_writes()
    -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?.with_target_control(recorder.clone());
        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/service-onboarding",
                ADMIN_KEY,
                r#"{
                    "service":"fast api",
                    "url":"https://example.com/fast/health",
                    "interval_ms":999
                }"#,
            )?)
            .await?;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(response).await?;
        assert_eq!(body["code"], "validation_error");
        assert_eq!(
            body["errors"]["interval_ms"],
            json!(["must be greater than or equal to 1000"])
        );
        let targets = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        assert_eq!(json_body(targets).await?["targets"], json!([]));
        assert!(recorder.commands().is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn service_onboarding_links_use_forwarded_proto_and_host_fallbacks()
    -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        let forwarded = router
            .clone()
            .oneshot(
                Request::post("/api/v1/service-onboarding")
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .header("host", "canary.example")
                    .header("x-forwarded-proto", "https")
                    .body(Body::from(
                        r#"{"service":"web api","url":"https://example.com/web/health"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(forwarded.status(), StatusCode::CREATED);
        let forwarded = json_body(forwarded).await?;
        assert_eq!(
            forwarded["links"]["report"],
            "https://canary.example/api/v1/report?window=1h"
        );
        assert!(
            forwarded["snippets"]["report_curl"]
                .as_str()
                .ok_or("missing report curl")?
                .contains("https://canary.example/api/v1/report?window=1h")
        );

        let fallback = router
            .oneshot(
                Request::post("/api/v1/service-onboarding")
                    .header("authorization", format!("Bearer {ADMIN_KEY}"))
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .header("x-forwarded-proto", "ftp")
                    .body(Body::from(
                        r#"{"service":"worker api","url":"https://example.com/worker/health"}"#,
                    ))?,
            )
            .await?;
        assert_eq!(fallback.status(), StatusCode::CREATED);
        let fallback = json_body(fallback).await?;
        assert_eq!(
            fallback["links"]["service_query"],
            "http://localhost/api/v1/query?service=worker+api&window=1h"
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
        assert!(recorder.commands().is_empty());

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
        let created = json_body(create_response).await?;
        let target_id = created["target"]["id"]
            .as_str()
            .ok_or("missing target id")?
            .to_owned();
        assert_eq!(
            recorder.commands(),
            vec![TargetProbeLifecycleCommand::Track {
                target_id,
                interval_ms: 60_000,
            }]
        );

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
        assert_eq!(duplicate["errors"].get("url"), None);

        let duplicate_url_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/service-onboarding",
                ADMIN_KEY,
                r#"{"service":"different-worker","url":"http://127.0.0.1:9/health","allow_private":true}"#,
            )?)
            .await?;
        assert_eq!(
            duplicate_url_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let duplicate_url = json_body(duplicate_url_response).await?;
        assert_eq!(
            duplicate_url["errors"]["url"],
            json!(["is already monitored"])
        );
        assert_eq!(duplicate_url["errors"].get("service"), None);

        let duplicate_service_and_url_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/service-onboarding",
                ADMIN_KEY,
                r#"{"service":"worker","url":"http://127.0.0.1:9/health","allow_private":true}"#,
            )?)
            .await?;
        assert_eq!(
            duplicate_service_and_url_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let duplicate_service_and_url = json_body(duplicate_service_and_url_response).await?;
        assert_eq!(
            duplicate_service_and_url["errors"]["service"],
            json!(["already has a health target"])
        );
        assert_eq!(
            duplicate_service_and_url["errors"]["url"],
            json!(["is already monitored"])
        );
        assert_eq!(recorder.commands().len(), 1);

        let keys_response = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/api/v1/keys")?)
            .await?;
        let keys_body = json_body(keys_response).await?;
        let worker_key_count = keys_body["keys"]
            .as_array()
            .ok_or("keys should be an array")?
            .iter()
            .filter(|key| key["name"] == "worker-ingest")
            .count();
        assert_eq!(worker_key_count, 1);

        let targets_response = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        let targets_body = json_body(targets_response).await?;
        let worker_target_count = targets_body["targets"]
            .as_array()
            .ok_or("targets should be an array")?
            .iter()
            .filter(|target| target["service"] == "worker")
            .count();
        assert_eq!(worker_target_count, 1);

        Ok(())
    }

    #[tokio::test]
    async fn service_onboarding_request_allow_private_cannot_override_server_policy()
    -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?.with_target_control(recorder.clone());
        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/service-onboarding",
                ADMIN_KEY,
                r#"{"service":"worker","url":"http://127.0.0.1:9/health","allow_private":true}"#,
            )?)
            .await?;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(response).await?;
        assert_eq!(body["detail"], "Invalid service onboarding request.");
        assert!(
            body["errors"]["url"][0].as_str().is_some_and(
                |message| message.contains("non-global") || message.contains("localhost")
            )
        );

        let targets = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        assert_eq!(json_body(targets).await?["targets"], json!([]));
        assert!(recorder.commands().is_empty());

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

        let too_fast_response = router
            .clone()
            .oneshot(json_request(
                "PATCH",
                &format!("/api/v1/targets/{target_id}"),
                ADMIN_KEY,
                r#"{"interval_ms":999}"#,
            )?)
            .await?;
        assert_eq!(too_fast_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let too_fast = json_body(too_fast_response).await?;
        assert_eq!(
            too_fast["errors"]["interval_ms"],
            json!(["must be greater than or equal to 1000"])
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
    async fn admin_target_create_rejects_subsecond_interval_without_writing_or_commanding()
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
                ADMIN_KEY,
                r#"{
                    "url":"http://127.0.0.1:9/health",
                    "name":"Local API",
                    "interval_ms":999,
                    "allow_private":true
                }"#,
            )?)
            .await?;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(response).await?;
        assert_eq!(
            body["errors"]["interval_ms"],
            json!(["must be greater than or equal to 1000"])
        );
        let targets = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        assert_eq!(json_body(targets).await?["targets"], json!([]));
        assert!(recorder.commands().is_empty());

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
    async fn admin_target_request_allow_private_cannot_override_server_policy()
    -> Result<(), Box<dyn Error>> {
        let recorder = Arc::new(RecordingTargetControl::default());
        let state = test_ingest_state()?.with_target_control(recorder.clone());
        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/targets",
                ADMIN_KEY,
                r#"{"url":"http://127.0.0.1:9/health","name":"Local API","allow_private":true}"#,
            )?)
            .await?;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(response).await?;
        assert_eq!(body["code"], "validation_error");
        assert!(
            body["detail"].as_str().is_some_and(
                |message| message.contains("non-global") || message.contains("localhost")
            )
        );

        let list = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        assert_eq!(json_body(list).await?["targets"], json!([]));
        assert!(recorder.commands().is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn admin_monitor_mutations_follow_contract() -> Result<(), Box<dyn Error>> {
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
    async fn admin_monitor_routes_reject_non_admin_scopes() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        let read_list_response = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/monitors")?)
            .await?;
        assert_eq!(read_list_response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            json_body(read_list_response).await?["detail"],
            "API key scope `read-only` cannot access this admin endpoint. Use an `admin` key."
        );

        let ingest_delete_response = router
            .clone()
            .oneshot(
                Request::delete("/api/v1/monitors/MON-missing")
                    .header("authorization", format!("Bearer {INGEST_KEY}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(ingest_delete_response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            json_body(ingest_delete_response).await?["detail"],
            "API key scope `ingest-only` cannot access this admin endpoint. Use an `admin` key."
        );

        let list_after_forbidden = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/monitors")?)
            .await?;
        assert_eq!(
            json_body(list_after_forbidden).await?["monitors"],
            json!([])
        );

        Ok(())
    }

    #[tokio::test]
    async fn admin_webhook_mutations_follow_contract() -> Result<(), Box<dyn Error>> {
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
            .clone()
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

        let local_webhook_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/webhooks",
                ADMIN_KEY,
                r#"{"url":"http://127.0.0.1:9/hook","events":["error.new_class"]}"#,
            )?)
            .await?;
        assert_eq!(
            local_webhook_response.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let local_webhook = json_body(local_webhook_response).await?;
        assert_eq!(local_webhook["code"], "validation_error");
        assert_eq!(local_webhook["detail"], "Invalid webhook configuration.");

        Ok(())
    }

    #[tokio::test]
    async fn admin_webhook_routes_reject_non_admin_scopes() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        let read_list_response = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/webhooks")?)
            .await?;
        assert_eq!(read_list_response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            json_body(read_list_response).await?["detail"],
            "API key scope `read-only` cannot access this admin endpoint. Use an `admin` key."
        );

        let ingest_delete_response = router
            .clone()
            .oneshot(
                Request::delete("/api/v1/webhooks/WHK-missing")
                    .header("authorization", format!("Bearer {INGEST_KEY}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(ingest_delete_response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            json_body(ingest_delete_response).await?["detail"],
            "API key scope `ingest-only` cannot access this admin endpoint. Use an `admin` key."
        );

        let list_after_forbidden = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/webhooks")?)
            .await?;
        assert_eq!(
            json_body(list_after_forbidden).await?["webhooks"],
            json!([])
        );

        Ok(())
    }

    #[tokio::test]
    async fn admin_webhook_test_delivery_maps_inactive_and_request_errors()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?.with_webhook_transport(Arc::new(
            RecordingTransport::request_error("connection refused"),
        ));
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            store.insert_webhook_subscription(webhook_subscription_insert(
                "WHK-inactive-test",
                "https://example.com/inactive",
                vec!["canary.ping".to_owned()],
                "inactive-secret",
                false,
                "2026-06-01T00:00:00Z",
            ))?;
        }
        let router = ingest_router(state);

        let inactive_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/webhooks/WHK-inactive-test/test",
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(inactive_response.status(), StatusCode::BAD_GATEWAY);
        let inactive_body = json_body(inactive_response).await?;
        assert_eq!(inactive_body["code"], "webhook_delivery_failed");
        assert_eq!(
            inactive_body["detail"],
            "Webhook test delivery failed: webhook_inactive"
        );

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
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/webhooks/{webhook_id}/test"),
                ADMIN_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(failed_response.status(), StatusCode::BAD_GATEWAY);
        let failed_body = json_body(failed_response).await?;
        assert_eq!(failed_body["code"], "webhook_delivery_failed");
        assert_eq!(
            failed_body["detail"],
            "Webhook test delivery failed: connection refused"
        );

        Ok(())
    }

    #[tokio::test]
    async fn admin_api_key_mutations_follow_contract() -> Result<(), Box<dyn Error>> {
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
        assert_eq!(created["service"], Value::Null);
        assert_eq!(created["tenant_id"], canary_store::BOOTSTRAP_TENANT_ID);
        assert_eq!(created["project_id"], canary_store::BOOTSTRAP_PROJECT_ID);
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
        assert_eq!(listed_key["service"], Value::Null);
        assert_eq!(listed_key["tenant_id"], canary_store::BOOTSTRAP_TENANT_ID);
        assert_eq!(listed_key["project_id"], canary_store::BOOTSTRAP_PROJECT_ID);
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

        let bound_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/keys",
                ADMIN_KEY,
                r#"{"name":"linejam-browser","scope":"ingest-only","service":"linejam"}"#,
            )?)
            .await?;
        assert_eq!(bound_response.status(), StatusCode::CREATED);
        let bound = json_body(bound_response).await?;
        assert_eq!(bound["scope"], "ingest-only");
        assert_eq!(bound["service"], "linejam");

        let invalid_bound_admin = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/keys",
                ADMIN_KEY,
                r#"{"scope":"admin","service":"linejam"}"#,
            )?)
            .await?;
        assert_eq!(
            invalid_bound_admin.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            json_body(invalid_bound_admin).await?["code"],
            "validation_error"
        );

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
    async fn auth_cache_serves_repeats_but_never_wrong_prefix_tokens() -> Result<(), Box<dyn Error>>
    {
        let router = ingest_router(test_ingest_state()?);

        // First authenticated read pays bcrypt and populates the auth cache.
        let first = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/incidents")?)
            .await?;
        assert_eq!(first.status(), StatusCode::OK);

        // Repeat auth with the same token must keep working (cache hit path).
        let second = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/incidents")?)
            .await?;
        assert_eq!(second.status(), StatusCode::OK);

        // A different token sharing the cached key's prefix must never be
        // served from the cache entry.
        let wrong = router
            .oneshot(read_request("sk_live_read_wrong", "/api/v1/incidents")?)
            .await?;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(json_body(wrong).await?["code"], "invalid_api_key");

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

        let responder_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/keys",
                ADMIN_KEY,
                r#"{"name":"bb-responder","scope":"responder-write","service":"bitterblossom"}"#,
            )?)
            .await?;
        assert_eq!(responder_response.status(), StatusCode::CREATED);
        let responder_key = json_body(responder_response).await?;
        assert_eq!(responder_key["scope"], "responder-write");
        assert_eq!(responder_key["service"], "bitterblossom");

        let unbound_responder = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/keys",
                ADMIN_KEY,
                r#"{"name":"bad-responder","scope":"responder-write"}"#,
            )?)
            .await?;
        assert_eq!(unbound_responder.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(unbound_responder).await?["errors"]["service"],
            json!(["is required for responder-write keys"])
        );

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
    async fn admin_api_key_routes_reject_non_admin_scopes() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        let missing_list_response = router
            .clone()
            .oneshot(Request::get("/api/v1/keys").body(Body::empty())?)
            .await?;
        assert_eq!(missing_list_response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(missing_list_response).await?["code"],
            "invalid_api_key"
        );

        let read_list_response = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/keys")?)
            .await?;
        assert_eq!(read_list_response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            json_body(read_list_response).await?["detail"],
            "API key scope `read-only` cannot access this admin endpoint. Use an `admin` key."
        );

        let ingest_revoke_response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/keys/KEY-missing/revoke",
                INGEST_KEY,
                "{}",
            )?)
            .await?;
        assert_eq!(ingest_revoke_response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            json_body(ingest_revoke_response).await?["detail"],
            "API key scope `ingest-only` cannot access this admin endpoint. Use an `admin` key."
        );

        let list_after_forbidden = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/keys")?)
            .await?;
        let keys = json_body(list_after_forbidden).await?["keys"]
            .as_array()
            .ok_or("keys should be an array")?
            .clone();
        assert!(keys.iter().all(|key| key["id"] != "KEY-missing"));

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
    async fn error_query_service_default_window_is_1h() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = router
            .oneshot(read_request(READ_KEY, "/api/v1/query?service=test-svc")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["service"], "test-svc");
        assert_eq!(body["window"], "1h");
        assert_eq!(body["total_errors"], 1);

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
    async fn service_bound_read_key_cannot_query_another_service() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let read_key = "sk_live_linejam_read_secret";
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            store.insert_api_key(ApiKeyInsert {
                id: "KEY-linejam-read".to_owned(),
                name: "linejam read".to_owned(),
                key_prefix: read_key.chars().take(API_KEY_PREFIX_LEN).collect(),
                key_hash: bcrypt::hash(read_key, TEST_BCRYPT_COST)?,
                created_at: "2026-05-28T20:00:00Z".to_owned(),
                revoked_at: None,
                scope: "read-only".to_owned(),
                tenant_id: "TENANT-alpha".to_owned(),
                project_id: "PROJECT-web".to_owned(),
                service: Some("linejam".to_owned()),
            })?;
            store.commit_error_ingest(owned_error_ingest(
                "TENANT-alpha",
                "PROJECT-web",
                "ERR-linejamread1",
                "EVT-linejamread1",
                "group-linejam-read",
                "linejam",
                "linejam visible token",
            )?)?;
        }
        let router = ingest_router(state);

        let forbidden = router
            .clone()
            .oneshot(read_request(read_key, "/api/v1/query?service=vanity")?)
            .await?;
        let status = forbidden.status();
        let body = json_body(forbidden).await?;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "insufficient_scope");
        assert_eq!(body["bound_service"], "linejam");
        assert_eq!(body["requested_service"], "vanity");

        let allowed = router
            .oneshot(read_request(
                read_key,
                "/api/v1/query?service=linejam&window=30d",
            )?)
            .await?;
        let status = allowed.status();
        let body = json_body(allowed).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["service"], "linejam");
        assert_eq!(body["total_errors"], 1);
        assert_eq!(body["groups"][0]["service"], "linejam");

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
                false,
            ),
            (
                read_request(READ_KEY, "/api/v1/query")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                false,
            ),
            (
                read_request(READ_KEY, "/api/v1/query?service=test-svc&window=99h")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                false,
            ),
            (
                read_request(READ_KEY, "/api/v1/query?service=test-svc&cursor=bogus")?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                true,
            ),
        ];

        for (request, expected_status, expected_code, expect_cursor_error) in cases {
            let response = ingest_router(test_ingest_state()?).oneshot(request).await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, expected_status);
            assert_eq!(body["code"], expected_code);
            if expect_cursor_error {
                assert_eq!(
                    body["errors"]["cursor"],
                    json!(["must be a valid pagination cursor"])
                );
            }
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
    async fn telemetry_event_ingest_correlates_to_timeline_and_report() -> Result<(), Box<dyn Error>>
    {
        let router = ingest_router(test_ingest_state()?);
        let response = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/events",
                INGEST_KEY,
                r#"{
                    "service":"checkout",
                    "name":"checkout.completed",
                    "summary":"Checkout completed",
                    "severity":"info",
                    "attributes":{"plan":"pro","amount":42},
                    "retention_class":"standard",
                    "privacy_policy":"redacted",
                    "sampling_policy":"sampled:0.25"
                }"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["event"], "telemetry.event");
        assert_eq!(body["name"], "checkout.completed");
        assert_eq!(body["attributes"]["plan"], "pro");

        let timeline = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/timeline?service=checkout&event_type=telemetry.event&limit=1",
            )?)
            .await?;
        let timeline_status = timeline.status();
        let timeline_body = json_body(timeline).await?;

        assert_eq!(timeline_status, StatusCode::OK);
        assert_eq!(timeline_body["returned_count"], 1);
        assert_eq!(timeline_body["events"][0]["event"], "telemetry.event");
        assert_eq!(timeline_body["events"][0]["signal_kind"], "analytics_event");
        assert_eq!(
            timeline_body["events"][0]["signal_name"],
            "checkout.completed"
        );
        assert_eq!(timeline_body["events"][0]["attributes"]["amount"], 42);
        assert_eq!(timeline_body["events"][0]["privacy_policy"], "redacted");

        let report = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/report?window=1h")?)
            .await?;
        let report_status = report.status();
        let report_body = json_body(report).await?;

        assert_eq!(report_status, StatusCode::OK);
        assert_eq!(
            report_body["recent_events"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            report_body["recent_events"][0]["signal_name"],
            "checkout.completed"
        );

        let invalid = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/events",
                INGEST_KEY,
                r#"{"service":"checkout","name":"","summary":"bad","attributes":[]}"#,
            )?)
            .await?;
        let invalid_status = invalid.status();
        let invalid_body = json_body(invalid).await?;
        assert_eq!(invalid_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(invalid_body["code"], "validation_error");
        assert_eq!(
            invalid_body["errors"]["attributes"],
            json!(["must be an object"])
        );

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
                "API key scope `ingest-only` cannot access this read endpoint. Use an `admin` or `read-only` or `responder-write` key.",
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
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            store.create_pending_webhook_delivery(webhook_delivery_insert(
                "DLV-old",
                "WHK-alpha",
                "error.new_class",
                "2026-04-02T10:00:00Z",
            ))?;
            store.mark_webhook_delivery_attempt("DLV-old", "2026-04-02T10:00:01Z")?;
            store.mark_webhook_delivery_delivered("DLV-old", "2026-04-02T10:00:02Z")?;
            store.create_suppressed_webhook_delivery(
                webhook_delivery_insert(
                    "DLV-suppressed",
                    "WHK-alpha",
                    "error.new_class",
                    "2026-04-02T10:05:00Z",
                ),
                "cooldown",
            )?;
            store.create_suppressed_webhook_delivery(
                webhook_delivery_insert(
                    "DLV-other",
                    "WHK-beta",
                    "incident.updated",
                    "2026-04-02T10:10:00Z",
                ),
                "cooldown",
            )?;
            store.create_pending_webhook_delivery(webhook_delivery_insert(
                "DLV-pending",
                "WHK-pending",
                "error.new_class",
                "2026-04-02T10:15:00Z",
            ))?;
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

        let event_filtered = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/webhook-deliveries?event=incident.updated",
            )?)
            .await?;
        let event_filtered_body = json_body(event_filtered).await?;
        assert_eq!(event_filtered_body["returned_count"], 1);
        assert_eq!(
            event_filtered_body["deliveries"][0]["delivery_id"],
            "DLV-other"
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
            .clone()
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

        let admin_read = router
            .oneshot(read_request(ADMIN_KEY, "/api/v1/webhook-deliveries")?)
            .await?;
        assert_eq!(admin_read.status(), StatusCode::OK);

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
                "API key scope `ingest-only` cannot access this read endpoint. Use an `admin` or `read-only` or `responder-write` key.",
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
            (
                read_request(
                    READ_KEY,
                    "/api/v1/webhook-deliveries?webhook_id%5B%5D=WHK-alpha",
                )?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "webhook_id",
                "must be a string",
            ),
            (
                read_request(
                    READ_KEY,
                    "/api/v1/webhook-deliveries?event%5B%5D=error.new_class",
                )?,
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "event",
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
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
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
    async fn webhook_delivery_show_returns_one_row_for_stable_delivery_id()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            store.create_pending_webhook_delivery(webhook_delivery_insert(
                "DLV-diagnostic-show",
                "WHK-diagnostic",
                "incident.updated",
                "2026-04-02T10:00:00Z",
            ))?;
            store.mark_webhook_delivery_attempt("DLV-diagnostic-show", "2026-04-02T10:00:01Z")?;
            store.mark_webhook_delivery_discarded(
                "DLV-diagnostic-show",
                "http_500",
                "2026-04-02T10:00:02Z",
            )?;
        }

        let response = ingest_router(state)
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/webhook-deliveries/DLV-diagnostic-show",
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["delivery_id"], "DLV-diagnostic-show");
        assert_eq!(body["webhook_id"], "WHK-diagnostic");
        assert_eq!(body["event"], "incident.updated");
        assert_eq!(body["status"], "discarded");
        assert_eq!(body["attempt_count"], 1);
        assert_eq!(body["reason"], "http_500");
        assert_eq!(body["last_attempt_at"], "2026-04-02T10:00:01Z");
        assert_eq!(body["discarded_at"], "2026-04-02T10:00:02Z");
        assert_eq!(body["completed_at"], "2026-04-02T10:00:02Z");

        Ok(())
    }

    #[tokio::test]
    async fn webhook_delivery_show_reports_missing_and_wrong_scope() -> Result<(), Box<dyn Error>> {
        let missing = ingest_router(test_ingest_state()?)
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/webhook-deliveries/DLV-missing",
            )?)
            .await?;
        let missing_status = missing.status();
        let missing_body = json_body(missing).await?;

        assert_eq!(missing_status, StatusCode::NOT_FOUND);
        assert_eq!(missing_body["code"], "not_found");
        assert_eq!(
            missing_body["detail"],
            "Webhook delivery DLV-missing not found."
        );

        let forbidden = ingest_router(test_ingest_state()?)
            .oneshot(read_request(
                INGEST_KEY,
                "/api/v1/webhook-deliveries/DLV-missing",
            )?)
            .await?;
        let forbidden_status = forbidden.status();
        let forbidden_body = json_body(forbidden).await?;

        assert_eq!(forbidden_status, StatusCode::FORBIDDEN);
        assert_eq!(forbidden_body["code"], "insufficient_scope");
        assert_eq!(
            forbidden_body["detail"],
            "API key scope `ingest-only` cannot access this read endpoint. Use an `admin` or `read-only` or `responder-write` key."
        );

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
    async fn read_routes_scope_health_status_and_webhook_deliveries_by_owner()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_api_key_with_owner(
                &mut store,
                "KEY-other-read",
                OTHER_READ_KEY,
                "read-only",
                None,
                "TENANT-other",
                "PROJECT-other",
            )?;
            let mut target = target_insert("other-api");
            target.id = "TGT-other-api".to_owned();
            store.insert_target_scoped(target, "TENANT-other", "PROJECT-other")?;
            store.create_pending_webhook_delivery(webhook_delivery_insert(
                "DLV-bootstrap",
                "WHK-bootstrap",
                "error.new_class",
                "2026-04-02T10:00:00Z",
            ))?;
            store.create_pending_webhook_delivery(webhook_delivery_insert_with_owner(
                "DLV-other-tenant",
                "WHK-other",
                "error.new_class",
                "2026-04-02T10:01:00Z",
                "TENANT-other",
                "PROJECT-other",
                Some("other-api"),
            ))?;
        }
        let router = ingest_router(state);

        let bootstrap_health = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/health-status")?)
            .await?;
        assert_eq!(json_body(bootstrap_health).await?["targets"], json!([]));

        let other_health = router
            .clone()
            .oneshot(read_request(OTHER_READ_KEY, "/api/v1/health-status")?)
            .await?;
        let other_health = json_body(other_health).await?;
        assert_eq!(other_health["targets"][0]["id"], "TGT-other-api");
        assert_eq!(other_health["targets"][0]["service"], "other-api");

        let bootstrap_deliveries = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/webhook-deliveries")?)
            .await?;
        let bootstrap_deliveries = json_body(bootstrap_deliveries).await?;
        assert_eq!(bootstrap_deliveries["returned_count"], 1);
        assert_eq!(
            bootstrap_deliveries["deliveries"][0]["delivery_id"],
            "DLV-bootstrap"
        );

        let other_deliveries = router
            .clone()
            .oneshot(read_request(OTHER_READ_KEY, "/api/v1/webhook-deliveries")?)
            .await?;
        let other_deliveries = json_body(other_deliveries).await?;
        assert_eq!(other_deliveries["returned_count"], 1);
        assert_eq!(
            other_deliveries["deliveries"][0]["delivery_id"],
            "DLV-other-tenant"
        );

        let hidden_delivery = router
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/webhook-deliveries/DLV-other-tenant",
            )?)
            .await?;
        assert_eq!(hidden_delivery.status(), StatusCode::NOT_FOUND);

        Ok(())
    }

    #[tokio::test]
    async fn service_bound_read_routes_hide_sibling_service_surfaces() -> Result<(), Box<dyn Error>>
    {
        const SERVICE_READ_KEY: &str = "sk_live_billing_read_secret";
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_read_api_key_for_service(
                &mut store,
                "KEY-billing-read",
                SERVICE_READ_KEY,
                "billing",
            )?;
            seed_target(&mut store, "billing")?;
            seed_target(&mut store, "payments")?;
            seed_monitor(&mut store, "billing")?;
            seed_monitor(&mut store, "payments")?;
            for service in ["billing", "payments"] {
                store.commit_target_probe(TargetProbeCommit {
                    target_id: format!("TGT-{service}"),
                    state: "up".to_owned(),
                    consecutive_failures: 0,
                    consecutive_successes: 1,
                    check_succeeded: true,
                    check: TargetCheckObservation {
                        status_code: Some(200),
                        latency_ms: Some(42),
                        result: "ok".to_owned(),
                        tls_expires_at: None,
                        error_detail: None,
                        region: None,
                    },
                    now: server_time::current_rfc3339(),
                    transition: None,
                })?;
            }
            store.create_pending_webhook_delivery(webhook_delivery_insert_with_owner(
                "DLV-billing",
                "WHK-billing",
                "error.new_class",
                "2026-04-02T10:00:00Z",
                canary_store::BOOTSTRAP_TENANT_ID,
                canary_store::BOOTSTRAP_PROJECT_ID,
                Some("billing"),
            ))?;
            store.create_pending_webhook_delivery(webhook_delivery_insert_with_owner(
                "DLV-payments",
                "WHK-payments",
                "error.new_class",
                "2026-04-02T10:01:00Z",
                canary_store::BOOTSTRAP_TENANT_ID,
                canary_store::BOOTSTRAP_PROJECT_ID,
                Some("payments"),
            ))?;
            for service in ["billing", "payments"] {
                store.create_annotation(AnnotationInsert {
                    id: format!("ANN-{service}"),
                    event_id: format!("EVT-ann-{service}"),
                    tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
                    project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
                    service: None,
                    subject_type: "target".to_owned(),
                    subject_id: format!("TGT-{service}"),
                    agent: "codex".to_owned(),
                    action: "triaged".to_owned(),
                    metadata: Some(json!({"service": service})),
                    created_at: "2026-04-02T10:02:00Z".to_owned(),
                })?;
            }
        }
        let router = ingest_router(state);

        let health = router
            .clone()
            .oneshot(read_request(SERVICE_READ_KEY, "/api/v1/health-status")?)
            .await?;
        let health = json_body(health).await?;
        assert_eq!(health["targets"].as_array().map(Vec::len), Some(1));
        assert_eq!(health["targets"][0]["service"], "billing");
        assert_eq!(health["monitors"].as_array().map(Vec::len), Some(1));
        assert_eq!(health["monitors"][0]["service"], "billing");

        let status = router
            .clone()
            .oneshot(read_request(SERVICE_READ_KEY, "/api/v1/status")?)
            .await?;
        let status = json_body(status).await?;
        assert_eq!(status["targets"].as_array().map(Vec::len), Some(1));
        assert_eq!(status["targets"][0]["id"], "TGT-billing");
        assert_eq!(status["monitors"].as_array().map(Vec::len), Some(1));
        assert_eq!(status["monitors"][0]["service"], "billing");

        let billing_checks = router
            .clone()
            .oneshot(read_request(
                SERVICE_READ_KEY,
                "/api/v1/targets/TGT-billing/checks",
            )?)
            .await?;
        let billing_checks = json_body(billing_checks).await?;
        assert_eq!(billing_checks["checks"][0]["result"], "ok");

        let payments_checks = router
            .clone()
            .oneshot(read_request(
                SERVICE_READ_KEY,
                "/api/v1/targets/TGT-payments/checks",
            )?)
            .await?;
        let payments_checks = json_body(payments_checks).await?;
        assert_eq!(payments_checks["checks"], json!([]));

        let deliveries = router
            .clone()
            .oneshot(read_request(
                SERVICE_READ_KEY,
                "/api/v1/webhook-deliveries",
            )?)
            .await?;
        let deliveries = json_body(deliveries).await?;
        assert_eq!(deliveries["returned_count"], 1);
        assert_eq!(deliveries["deliveries"][0]["delivery_id"], "DLV-billing");

        let hidden_delivery = router
            .clone()
            .oneshot(read_request(
                SERVICE_READ_KEY,
                "/api/v1/webhook-deliveries/DLV-payments",
            )?)
            .await?;
        assert_eq!(hidden_delivery.status(), StatusCode::NOT_FOUND);

        let billing_annotations = router
            .clone()
            .oneshot(read_request(
                SERVICE_READ_KEY,
                "/api/v1/annotations?subject_type=target&subject_id=TGT-billing",
            )?)
            .await?;
        let billing_annotations = json_body(billing_annotations).await?;
        assert_eq!(
            billing_annotations["annotations"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            billing_annotations["annotations"][0]["subject_id"],
            "TGT-billing"
        );

        let hidden_annotations = router
            .oneshot(read_request(
                SERVICE_READ_KEY,
                "/api/v1/annotations?subject_type=target&subject_id=TGT-payments",
            )?)
            .await?;
        assert_eq!(hidden_annotations.status(), StatusCode::NOT_FOUND);

        Ok(())
    }

    #[tokio::test]
    async fn service_bound_admin_key_is_rejected_even_if_persisted() -> Result<(), Box<dyn Error>> {
        const SERVICE_ADMIN_KEY: &str = "sk_live_billing_admin_secret";
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_admin_api_key_for_service(
                &mut store,
                "KEY-billing-admin",
                SERVICE_ADMIN_KEY,
                "billing",
            )?;
        }

        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(read_request(SERVICE_ADMIN_KEY, "/api/v1/targets")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "insufficient_scope");
        assert_eq!(body["bound_service"], "billing");
        assert_eq!(body["requested_service"], "*");

        let response = router
            .clone()
            .oneshot(read_request(
                SERVICE_ADMIN_KEY,
                "/api/v1/query?service=billing",
            )?)
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = router
            .oneshot(error_request(
                SERVICE_ADMIN_KEY,
                r#"{"service":"billing","error_class":"RuntimeError","message":"boom"}"#,
            )?)
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

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
    async fn health_read_routes_reject_ingest_scope() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);

        for path in [
            "/api/v1/health-status",
            "/api/v1/status",
            "/api/v1/targets/TGT-any/checks",
        ] {
            let response = router
                .clone()
                .oneshot(read_request(INGEST_KEY, path)?)
                .await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
            assert_eq!(body["code"], "insufficient_scope", "{path}");
            assert_eq!(
                body["detail"],
                "API key scope `ingest-only` cannot access this read endpoint. Use an `admin` or `read-only` or `responder-write` key.",
                "{path}"
            );
        }

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
    async fn read_routes_scope_query_report_and_error_detail_by_owner() -> Result<(), Box<dyn Error>>
    {
        let state = test_ingest_state()?;
        let bootstrap_error = owned_error_ingest(
            canary_store::BOOTSTRAP_TENANT_ID,
            canary_store::BOOTSTRAP_PROJECT_ID,
            "ERR-bootread1234",
            "EVT-bootread1234",
            "group-bootstrap-scope",
            "shared-api",
            "bootstrap visible token",
        )?;
        let other_error = owned_error_ingest(
            "TENANT-other",
            "PROJECT-other",
            "ERR-otherread123",
            "EVT-otherread123",
            "group-other-scope",
            "shared-api",
            "other tenant token",
        )?;
        let other_error_id = other_error.ids.error_id.to_string();
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_api_key_with_owner(
                &mut store,
                "KEY-other-read",
                OTHER_READ_KEY,
                "read-only",
                None,
                "TENANT-other",
                "PROJECT-other",
            )?;
            store.commit_error_ingest(bootstrap_error)?;
            store.commit_error_ingest(other_error)?;
        }
        let router = ingest_router(state);

        let bootstrap_query = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/query?service=shared-api&window=30d",
            )?)
            .await?;
        let bootstrap_query = json_body(bootstrap_query).await?;
        assert_eq!(
            bootstrap_query["groups"][0]["group_hash"],
            "group-bootstrap-scope"
        );
        assert_eq!(bootstrap_query["total_errors"], 1);

        let other_query = router
            .clone()
            .oneshot(read_request(
                OTHER_READ_KEY,
                "/api/v1/query?service=shared-api&window=30d",
            )?)
            .await?;
        let other_query = json_body(other_query).await?;
        assert_eq!(other_query["groups"][0]["group_hash"], "group-other-scope");
        assert_eq!(other_query["total_errors"], 1);

        let hidden_error = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                &format!("/api/v1/errors/{other_error_id}"),
            )?)
            .await?;
        assert_eq!(hidden_error.status(), StatusCode::NOT_FOUND);

        let visible_error = router
            .clone()
            .oneshot(read_request(
                OTHER_READ_KEY,
                &format!("/api/v1/errors/{other_error_id}"),
            )?)
            .await?;
        let visible_error = json_body(visible_error).await?;
        assert_eq!(visible_error["group_hash"], "group-other-scope");
        assert_eq!(visible_error["message"], "other tenant token");

        let bootstrap_report = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/report?window=30d&q=other%20tenant%20token",
            )?)
            .await?;
        let bootstrap_report = json_body(bootstrap_report).await?;
        assert_eq!(
            bootstrap_report["error_groups"][0]["group_hash"],
            "group-bootstrap-scope"
        );
        assert_eq!(
            bootstrap_report["service_sli"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(bootstrap_report["service_sli"][0]["service"], "shared-api");
        assert_eq!(bootstrap_report["service_sli"][0]["errors"]["total"], 1);
        assert_eq!(bootstrap_report["search_results"], json!([]));

        let other_report = router
            .oneshot(read_request(
                OTHER_READ_KEY,
                "/api/v1/report?window=30d&q=other%20tenant%20token",
            )?)
            .await?;
        let other_report = json_body(other_report).await?;
        assert_eq!(
            other_report["error_groups"][0]["group_hash"],
            "group-other-scope"
        );
        assert_eq!(
            other_report["service_sli"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(other_report["service_sli"][0]["service"], "shared-api");
        assert_eq!(other_report["service_sli"][0]["errors"]["total"], 1);
        assert_eq!(other_report["search_results"][0]["id"], other_error_id);

        Ok(())
    }

    #[tokio::test]
    async fn service_bound_read_key_filters_report_to_its_service() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let read_key = "sk_live_report_bound_secret";
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            store.insert_api_key(ApiKeyInsert {
                id: "KEY-report-bound".to_owned(),
                name: "report bound".to_owned(),
                key_prefix: read_key.chars().take(API_KEY_PREFIX_LEN).collect(),
                key_hash: bcrypt::hash(read_key, TEST_BCRYPT_COST)?,
                created_at: "2026-05-28T20:00:00Z".to_owned(),
                revoked_at: None,
                scope: "read-only".to_owned(),
                tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
                project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
                service: Some("linejam".to_owned()),
            })?;
        }
        let router = ingest_router(state);

        for service in ["linejam", "vanity"] {
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

        let response = router
            .oneshot(read_request(
                read_key,
                "/api/v1/report?window=1h&q=timeout",
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["error_groups"].as_array().map(Vec::len), Some(1));
        assert_eq!(body["error_groups"][0]["service"], "linejam");
        assert_eq!(body["search_results"].as_array().map(Vec::len), Some(1));
        assert_eq!(body["search_results"][0]["service"], "linejam");

        Ok(())
    }

    #[tokio::test]
    async fn report_includes_windowed_service_sli_and_applies_service_binding()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let read_key = "sk_live_report_sli_api_secret";
        let now = server_time::current_rfc3339();
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_read_api_key_for_service(&mut store, "KEY-report-sli-api", read_key, "api")?;
            seed_target(&mut store, "api")?;
            seed_target(&mut store, "worker")?;
            seed_monitor(&mut store, "api")?;
            seed_monitor(&mut store, "worker")?;
            for (service, result, check_succeeded) in
                [("api", "success", true), ("worker", "error", false)]
            {
                store.commit_target_probe(TargetProbeCommit {
                    target_id: format!("TGT-{service}"),
                    state: if check_succeeded { "up" } else { "down" }.to_owned(),
                    consecutive_failures: if check_succeeded { 0 } else { 1 },
                    consecutive_successes: if check_succeeded { 1 } else { 0 },
                    check_succeeded,
                    check: TargetCheckObservation {
                        status_code: if check_succeeded {
                            Some(200)
                        } else {
                            Some(500)
                        },
                        latency_ms: Some(42),
                        result: result.to_owned(),
                        tls_expires_at: None,
                        error_detail: None,
                        region: None,
                    },
                    now: now.clone(),
                    transition: None,
                })?;
            }
            for (service, status) in [("api", "alive"), ("worker", "error")] {
                store.commit_monitor_check_in(MonitorCheckInCommit {
                    monitor_id: format!("MON-{service}"),
                    state: if status == "error" { "down" } else { "up" }.to_owned(),
                    last_check_in_at: Some(now.clone()),
                    last_check_in_status: Some(status.to_owned()),
                    deadline_at: Some(now.clone()),
                    check_in: MonitorCheckInObservation {
                        id: format!("CHK-{service}-sli"),
                        external_id: None,
                        status: status.to_owned(),
                        observed_at: now.clone(),
                        ttl_ms: None,
                        summary: None,
                        context: None,
                    },
                    now: now.clone(),
                    transition: None,
                })?;
            }
            let mut api_error = owned_error_ingest(
                canary_store::BOOTSTRAP_TENANT_ID,
                canary_store::BOOTSTRAP_PROJECT_ID,
                "ERR-sliapi000001",
                "EVT-sliapi000001",
                "group-report-sli-api",
                "api",
                "api timeout",
            )?;
            api_error.payload.created_at = now.clone();
            store.commit_error_ingest(api_error)?;
            let mut worker_error = owned_error_ingest(
                canary_store::BOOTSTRAP_TENANT_ID,
                canary_store::BOOTSTRAP_PROJECT_ID,
                "ERR-sliworker001",
                "EVT-sliworker001",
                "group-report-sli-worker",
                "worker",
                "worker timeout",
            )?;
            worker_error.payload.created_at = now.clone();
            store.commit_error_ingest(worker_error)?;
            store.correlate_incident(IncidentCorrelation {
                tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
                project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
                signal_type: "error_group".to_owned(),
                signal_ref: "group-report-sli-api".to_owned(),
                service: "api".to_owned(),
                incident_id: IncidentId::from_str("INC-sliapi000001")?,
                event_id: EventId::from_str("EVT-sliinc000001")?,
                now: now.clone(),
            })?;
        }
        let router = ingest_router(state);

        let report = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/report?window=1h")?)
            .await?;
        let report = json_body(report).await?;
        let service_sli = report["service_sli"]
            .as_array()
            .ok_or("report should include service_sli")?;
        assert_eq!(service_sli.len(), 2);
        let api = service_sli
            .iter()
            .find(|summary| summary["service"] == "api")
            .ok_or("missing api SLI")?;
        assert_eq!(api["window"], "1h");
        assert_eq!(api["slo"]["class"], "standard");
        assert_eq!(api["slo"]["source"], "default_health_surface");
        assert_eq!(api["slo"]["availability_target"], 0.995);
        assert_eq!(api["slo"]["latency_ms_average_target"], 1_000);
        assert_eq!(api["slo"]["error_budget_events_per_hour"], 5);
        assert_eq!(api["targets"]["checks"], 1);
        assert_eq!(api["targets"]["availability_ratio"], 1.0);
        assert_eq!(api["monitors"]["healthy_check_ins"], 1);
        assert_eq!(api["errors"]["total"], 1);
        assert_eq!(api["incidents"]["active"], 1);
        // One sample per window is below the trajectory sample floor, so the
        // availability delta is nulled while the exact error-count delta stands.
        assert_eq!(api["trajectory"]["status"], "insufficient_samples");
        assert!(api["trajectory"]["targets"]["availability_delta"].is_null());
        assert_eq!(api["trajectory"]["errors"]["total_delta"], 1);
        assert_eq!(api["trajectory"]["errors"]["prior_total"], 0);

        let bound = router
            .oneshot(read_request(read_key, "/api/v1/report?window=1h")?)
            .await?;
        let bound = json_body(bound).await?;
        assert_eq!(bound["service_sli"].as_array().map(Vec::len), Some(1));
        assert_eq!(bound["service_sli"][0]["service"], "api");

        Ok(())
    }

    #[tokio::test]
    async fn report_defaults_window_to_1h_and_rejects_invalid_window() -> Result<(), Box<dyn Error>>
    {
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            store.commit_error_ingest(test_error_ingest(1, "2026-04-01T00:00:00Z"))?;
        }
        let router = ingest_router(state);

        let default_window = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/report")?)
            .await?;
        let default_body = json_body(default_window).await?;
        assert_eq!(default_body["status"], "empty");
        assert_eq!(default_body["summary"], "No services configured.");
        assert_eq!(
            default_body["error_groups"].as_array().map(Vec::len),
            Some(0)
        );

        let invalid_window = router
            .oneshot(read_request(READ_KEY, "/api/v1/report?window=99h")?)
            .await?;
        let invalid_status = invalid_window.status();
        let invalid_body = json_body(invalid_window).await?;

        assert_eq!(invalid_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(invalid_body["code"], "validation_error");
        assert_eq!(
            invalid_body["errors"]["window"],
            json!(["must be one of: 1h, 6h, 24h, 7d, 30d"])
        );

        Ok(())
    }

    #[tokio::test]
    async fn report_paginates_targets_monitors_and_error_groups_independently()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            for service in [
                "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf",
            ] {
                seed_monitor(&mut store, service)?;
            }
        }
        let router = ingest_router(state);

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
            .oneshot(read_request(READ_KEY, "/api/v1/report?window=1h&limit=5")?)
            .await?;
        let first_body = json_body(first).await?;
        assert_eq!(first_body["targets"].as_array().map(Vec::len), Some(5));
        assert_eq!(first_body["monitors"].as_array().map(Vec::len), Some(5));
        assert_eq!(first_body["error_groups"].as_array().map(Vec::len), Some(5));
        let cursor = first_body["cursor"]
            .as_str()
            .ok_or("first report should return cursor")?;

        let second = router
            .oneshot(read_request(
                READ_KEY,
                &format!("/api/v1/report?window=1h&limit=5&cursor={cursor}"),
            )?)
            .await?;
        let second_body = json_body(second).await?;

        assert_eq!(second_body["targets"].as_array().map(Vec::len), Some(2));
        assert_eq!(second_body["monitors"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            second_body["error_groups"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(second_body["truncated"], false);
        assert_eq!(second_body["cursor"], Value::Null);

        Ok(())
    }

    #[tokio::test]
    async fn metrics_requires_admin_scope_and_returns_prometheus_snapshot()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_target(&mut store, "metrics-svc")?;
            seed_monitor(&mut store, "metrics-monitor")?;
            store.create_pending_webhook_delivery(webhook_delivery_insert(
                "DLV-metrics",
                "WHK-metrics",
                "error.new_class",
                "2026-05-28T20:00:00Z",
            ))?;
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
            Some(&HeaderValue::from_static(
                metrics_routes::PROMETHEUS_CONTENT_TYPE
            ))
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
        assert_eq!(
            forbidden.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static(
                http_contract::PROBLEM_CONTENT_TYPE
            ))
        );
        let forbidden_body = json_body(forbidden).await?;
        assert_eq!(forbidden_status, StatusCode::FORBIDDEN);
        assert_eq!(forbidden_body["code"], "insufficient_scope");
        assert_eq!(forbidden_body["status"], StatusCode::FORBIDDEN.as_u16());

        let unauthorized = ingest_router(state)
            .oneshot(Request::get("/metrics").body(Body::empty())?)
            .await?;
        let unauthorized_status = unauthorized.status();
        assert_eq!(
            unauthorized.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static(
                http_contract::PROBLEM_CONTENT_TYPE
            ))
        );
        let unauthorized_body = json_body(unauthorized).await?;
        assert_eq!(unauthorized_status, StatusCode::UNAUTHORIZED);
        assert_eq!(unauthorized_body["code"], "invalid_api_key");
        assert_eq!(
            unauthorized_body["status"],
            StatusCode::UNAUTHORIZED.as_u16()
        );

        Ok(())
    }

    #[tokio::test]
    async fn metrics_uses_query_rate_limit_after_admin_auth() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut limiter = state
                .rate_limiter()
                .lock()
                .map_err(|_| "rate limiter lock poisoned")?;
            for _ in 0..30 {
                assert_eq!(
                    limiter.check(RateLimitKind::Query, "KEY-admin"),
                    RateLimitDecision::Allowed
                );
            }
        }
        let router = ingest_router(state);

        let response = router
            .clone()
            .oneshot(read_request(ADMIN_KEY, "/metrics")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], "rate_limited");
        let retry_after = body["retry_after"]
            .as_u64()
            .ok_or("retry_after should be a number")?;
        assert!((1..=60).contains(&retry_after));

        let forbidden = router
            .clone()
            .oneshot(read_request(READ_KEY, "/metrics")?)
            .await?;
        let forbidden_status = forbidden.status();
        let forbidden_body = json_body(forbidden).await?;
        assert_eq!(forbidden_status, StatusCode::FORBIDDEN);
        assert_eq!(forbidden_body["code"], "insufficient_scope");

        let unauthorized = router
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
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
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
                now: server_time::current_rfc3339(),
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
    async fn target_checks_keeps_error_and_empty_missing_target_behavior()
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
                "API key scope `ingest-only` cannot access this read endpoint. Use an `admin` or `read-only` or `responder-write` key.",
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
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
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
                            "tenant_id": canary_store::BOOTSTRAP_TENANT_ID,
                            "project_id": canary_store::BOOTSTRAP_PROJECT_ID,
                            "service": "api",
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
    async fn remediation_claim_routes_coordinate_agents_and_emit_webhook_effects()
    -> Result<(), Box<dyn Error>> {
        const API_READ_KEY: &str = "sk_live_claims_api_read_secret";
        const WEB_READ_KEY: &str = "sk_live_claims_web_read_secret";
        let sink = Arc::new(RecordingFailingSink::default());
        let state = test_ingest_state_with_sink(sink.clone())?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_read_api_key_for_service(&mut store, "KEY-claims-api-read", API_READ_KEY, "api")?;
            seed_read_api_key_for_service(&mut store, "KEY-claims-web-read", WEB_READ_KEY, "web")?;
            seed_target(&mut store, "api")?;
        }
        let router = ingest_router(state);

        let created = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/claims",
                ADMIN_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-api","owner":"codex","purpose":"investigate failed deploy","ttl_ms":900000,"idempotency_key":"run-1","evidence_links":["https://example.com/run/1"]}"#,
            )?)
            .await?;
        let created_status = created.status();
        let created_body = json_body(created).await?;
        assert_eq!(created_status, StatusCode::CREATED);
        assert!(
            created_body["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("CLM-"))
        );
        assert_eq!(created_body["subject_type"], "target");
        assert_eq!(created_body["subject_id"], "TGT-api");
        assert_eq!(created_body["owner"], "codex");
        assert_eq!(created_body["state"], "claimed");
        let claim_id = created_body["id"].as_str().ok_or("missing claim id")?;

        let replayed = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/claims",
                ADMIN_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-api","owner":"codex","purpose":"investigate failed deploy","ttl_ms":900000,"idempotency_key":"run-1","evidence_links":["https://example.com/run/1"]}"#,
            )?)
            .await?;
        let replayed_status = replayed.status();
        let replayed_body = json_body(replayed).await?;
        assert_eq!(replayed_status, StatusCode::OK);
        assert_eq!(replayed_body["id"], claim_id);

        let shown = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                &format!("/api/v1/claims/{claim_id}"),
            )?)
            .await?;
        let shown_body = json_body(shown).await?;
        assert_eq!(shown_body["id"], claim_id);
        assert_eq!(shown_body["owner"], "codex");

        let conflict = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/claims",
                ADMIN_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-api","owner":"claude","purpose":"parallel triage","ttl_ms":900000,"idempotency_key":"run-2"}"#,
            )?)
            .await?;
        let conflict_status = conflict.status();
        let conflict_body = json_body(conflict).await?;
        assert_eq!(conflict_status, StatusCode::CONFLICT);
        assert_eq!(conflict_body["code"], "claim_conflict");
        assert_eq!(
            conflict_body["detail"],
            "Subject already has an active remediation claim. Release or complete the current claim before creating another active claim."
        );
        assert_eq!(conflict_body["current_claim"]["id"], claim_id);
        assert_eq!(conflict_body["current_claim"]["owner"], "codex");

        let listed = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/claims?subject_type=target&subject_id=TGT-api",
            )?)
            .await?;
        let listed_body = json_body(listed).await?;
        assert_eq!(listed_body["current_claim"]["id"], claim_id);
        assert_eq!(listed_body["claims"].as_array().map(Vec::len), Some(1));

        let service_visible = router
            .clone()
            .oneshot(read_request(
                API_READ_KEY,
                "/api/v1/claims?subject_type=target&subject_id=TGT-api",
            )?)
            .await?;
        assert_eq!(service_visible.status(), StatusCode::OK);

        let service_hidden = router
            .clone()
            .oneshot(read_request(
                WEB_READ_KEY,
                "/api/v1/claims?subject_type=target&subject_id=TGT-api",
            )?)
            .await?;
        assert_eq!(service_hidden.status(), StatusCode::NOT_FOUND);

        let annotation_page = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/annotations?subject_type=target&subject_id=TGT-api",
            )?)
            .await?;
        let annotation_page = json_body(annotation_page).await?;
        assert_eq!(annotation_page["current_claim"]["id"], claim_id);

        let error = router
            .clone()
            .oneshot(error_request(
                INGEST_KEY,
                r#"{"service":"api","error_class":"RuntimeError","message":"claim surface regression"}"#,
            )?)
            .await?;
        assert_eq!(error.status(), StatusCode::CREATED);
        let error_body = json_body(error).await?;
        let group_hash = error_body["group_hash"]
            .as_str()
            .ok_or("missing error group hash")?
            .to_owned();

        let group_claim = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/claims",
                ADMIN_KEY,
                &format!(
                    r#"{{"subject_type":"error_group","subject_id":"{}","owner":"codex","purpose":"inspect grouped errors","ttl_ms":900000,"idempotency_key":"run-group"}}"#,
                    group_hash
                ),
            )?)
            .await?;
        let group_claim_status = group_claim.status();
        let group_claim_body = json_body(group_claim).await?;
        assert_eq!(
            group_claim_status,
            StatusCode::CREATED,
            "{group_claim_body}"
        );
        let group_claim_id = group_claim_body["id"]
            .as_str()
            .ok_or("missing group claim")?;

        let query = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/query?service=api&window=30d",
            )?)
            .await?;
        let query = json_body(query).await?;
        assert_eq!(query["groups"][0]["group_hash"], group_hash);
        assert_eq!(query["groups"][0]["current_claim"]["id"], group_claim_id);

        let report = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/report?window=30d")?)
            .await?;
        let report = json_body(report).await?;
        assert_eq!(report["error_groups"][0]["group_hash"], group_hash);
        assert_eq!(
            report["error_groups"][0]["current_claim"]["id"],
            group_claim_id
        );

        let transitioned = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/claims/{claim_id}/transition"),
                ADMIN_KEY,
                r#"{"owner":"codex","state":"investigating","evidence_links":["https://example.com/run/2"]}"#,
            )?)
            .await?;
        let transitioned_body = json_body(transitioned).await?;
        assert_eq!(transitioned_body["state"], "investigating");
        assert_eq!(
            transitioned_body["evidence_links"],
            json!(["https://example.com/run/1", "https://example.com/run/2"])
        );

        let wrong_owner = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/claims/{claim_id}/transition"),
                ADMIN_KEY,
                r#"{"owner":"claude","state":"fix_proposed"}"#,
            )?)
            .await?;
        assert_eq!(wrong_owner.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let released = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/claims/{claim_id}/release"),
                ADMIN_KEY,
                r#"{"owner":"codex"}"#,
            )?)
            .await?;
        let released_body = json_body(released).await?;
        assert_eq!(released_body["state"], "released");

        {
            let effects = sink.effects.lock().map_err(|_| "effect lock poisoned")?;
            let events = effects
                .iter()
                .filter_map(|effect| match effect {
                    IngestEffect::EnqueueWebhook { event, .. }
                        if event.starts_with("remediation_claim.") =>
                    {
                        Some(event.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                events,
                [
                    "remediation_claim.created",
                    "remediation_claim.created",
                    "remediation_claim.updated",
                    "remediation_claim.released"
                ]
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn responder_write_key_claims_and_annotates_only_bound_service()
    -> Result<(), Box<dyn Error>> {
        const API_RESPONDER_KEY: &str = "sk_live_claims_api_responder_secret";
        const WEB_RESPONDER_KEY: &str = "sk_live_claims_web_responder_secret";
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_responder_api_key_for_service(
                &mut store,
                "KEY-api-responder",
                API_RESPONDER_KEY,
                "api",
            )?;
            seed_responder_api_key_for_service(
                &mut store,
                "KEY-web-responder",
                WEB_RESPONDER_KEY,
                "web",
            )?;
            seed_target(&mut store, "api")?;
            seed_target(&mut store, "web")?;
        }
        let router = ingest_router(state);

        let created = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/claims",
                API_RESPONDER_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-api","owner":"codex","purpose":"responder loop","ttl_ms":900000,"idempotency_key":"api-run"}"#,
            )?)
            .await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        let created_body = json_body(created).await?;
        assert_eq!(created_body["service"], "api");
        let claim_id = created_body["id"]
            .as_str()
            .ok_or("missing claim id")?
            .to_owned();

        let visible = router
            .clone()
            .oneshot(read_request(
                API_RESPONDER_KEY,
                &format!("/api/v1/claims/{claim_id}"),
            )?)
            .await?;
        assert_eq!(visible.status(), StatusCode::OK);

        let hidden = router
            .clone()
            .oneshot(read_request(
                WEB_RESPONDER_KEY,
                &format!("/api/v1/claims/{claim_id}"),
            )?)
            .await?;
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

        let cross_claim = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/claims",
                API_RESPONDER_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-web","owner":"codex","purpose":"wrong service","ttl_ms":900000,"idempotency_key":"web-run"}"#,
            )?)
            .await?;
        assert_eq!(cross_claim.status(), StatusCode::NOT_FOUND);

        let transitioned = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/claims/{claim_id}/transition"),
                API_RESPONDER_KEY,
                r#"{"owner":"codex","state":"verified","evidence_links":["https://example.com/proof"]}"#,
            )?)
            .await?;
        assert_eq!(transitioned.status(), StatusCode::OK);
        assert_eq!(json_body(transitioned).await?["state"], "verified");

        let annotation = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/annotations",
                API_RESPONDER_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-api","agent":"codex","action":"fix-verified","metadata":{"claim_id":"CLM-test"}}"#,
            )?)
            .await?;
        assert_eq!(annotation.status(), StatusCode::CREATED);
        let annotation_body = json_body(annotation).await?;
        assert_eq!(annotation_body["subject_id"], "TGT-api");
        assert_eq!(annotation_body["action"], "fix-verified");

        let cross_annotation = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/annotations",
                API_RESPONDER_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-web","agent":"codex","action":"fix-verified"}"#,
            )?)
            .await?;
        assert_eq!(cross_annotation.status(), StatusCode::FORBIDDEN);
        let cross_body = json_body(cross_annotation).await?;
        assert_eq!(cross_body["code"], "insufficient_scope");
        assert_eq!(cross_body["bound_service"], "api");
        assert_eq!(cross_body["requested_service"], "web");

        Ok(())
    }

    #[tokio::test]
    async fn annotation_routes_scope_subjects_and_rows_by_owner() -> Result<(), Box<dyn Error>> {
        let sink = Arc::new(RecordingFailingSink::default());
        let state = test_ingest_state_with_sink(sink.clone())?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_api_key_with_owner(
                &mut store,
                "KEY-other-admin",
                OTHER_ADMIN_KEY,
                "admin",
                None,
                "TENANT-other",
                "PROJECT-other",
            )?;
            seed_api_key_with_owner(
                &mut store,
                "KEY-other-read",
                OTHER_READ_KEY,
                "read-only",
                None,
                "TENANT-other",
                "PROJECT-other",
            )?;
            let mut target = target_insert("other-api");
            target.id = "TGT-other-api".to_owned();
            store.insert_target_scoped(target, "TENANT-other", "PROJECT-other")?;
        }
        let router = ingest_router(state);

        let hidden_create = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/annotations",
                ADMIN_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-other-api","agent":"bootstrap","action":"acknowledged"}"#,
            )?)
            .await?;
        assert_eq!(hidden_create.status(), StatusCode::NOT_FOUND);

        let created = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/annotations",
                OTHER_ADMIN_KEY,
                r#"{"subject_type":"target","subject_id":"TGT-other-api","agent":"other","action":"acknowledged"}"#,
            )?)
            .await?;
        let created_status = created.status();
        let created_body = json_body(created).await?;
        assert_eq!(created_status, StatusCode::CREATED);
        assert_eq!(created_body["subject_id"], "TGT-other-api");

        let hidden_list = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/annotations?subject_type=target&subject_id=TGT-other-api",
            )?)
            .await?;
        assert_eq!(hidden_list.status(), StatusCode::NOT_FOUND);

        let visible_list = router
            .clone()
            .oneshot(read_request(
                OTHER_READ_KEY,
                "/api/v1/annotations?subject_type=target&subject_id=TGT-other-api",
            )?)
            .await?;
        let visible_body = json_body(visible_list).await?;
        assert_eq!(
            visible_body["annotations"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(visible_body["annotations"][0]["agent"], "other");

        let effects = sink.effects.lock().map_err(|_| "effect lock poisoned")?;
        assert_eq!(effects.len(), 1);
        let IngestEffect::EnqueueWebhook { payload_json, .. } = &effects[0] else {
            return Err("expected annotation webhook".into());
        };
        let payload: Value = serde_json::from_str(payload_json)?;
        assert_eq!(payload["tenant_id"], "TENANT-other");
        assert_eq!(payload["project_id"], "PROJECT-other");

        Ok(())
    }

    #[tokio::test]
    async fn legacy_annotation_routes_and_errors_follow_contract() -> Result<(), Box<dyn Error>> {
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
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            let id = canary_core::ids::IncidentId::generate().into_string();
            store.correlate_incident(IncidentCorrelation {
                tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
                project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
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
    async fn incidents_filters_with_annotation_and_without_annotation_are_applied()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let router = ingest_router(state.clone());

        let first_error = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/errors",
                INGEST_KEY,
                r#"{"service":"api","error_class":"RuntimeError","message":"first"}"#,
            )?)
            .await?;
        let first_body = json_body(first_error).await?;
        let first_group = first_body["group_hash"]
            .as_str()
            .ok_or("missing first group hash")?
            .to_owned();

        let second_error = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/errors",
                INGEST_KEY,
                r#"{"service":"web","error_class":"RuntimeError","message":"second"}"#,
            )?)
            .await?;
        let second_body = json_body(second_error).await?;
        let second_group = second_body["group_hash"]
            .as_str()
            .ok_or("missing second group hash")?
            .to_owned();

        let annotated_incident_id = {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            let annotated_id = canary_core::ids::IncidentId::generate().into_string();
            store.correlate_incident(IncidentCorrelation {
                tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
                project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
                signal_type: "error_group".to_owned(),
                signal_ref: first_group,
                service: "api".to_owned(),
                incident_id: annotated_id.parse()?,
                event_id: canary_core::ids::EventId::generate(),
                now: "2026-05-28T20:00:00Z".to_owned(),
            })?;
            let plain_id = canary_core::ids::IncidentId::generate().into_string();
            store.correlate_incident(IncidentCorrelation {
                tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
                project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
                signal_type: "error_group".to_owned(),
                signal_ref: second_group,
                service: "web".to_owned(),
                incident_id: plain_id.parse()?,
                event_id: canary_core::ids::EventId::generate(),
                now: "2026-05-28T20:00:01Z".to_owned(),
            })?;
            annotated_id
        };

        let annotation = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{annotated_incident_id}/annotations"),
                ADMIN_KEY,
                r#"{"agent":"triage-bot","action":"acknowledged"}"#,
            )?)
            .await?;
        assert_eq!(annotation.status(), StatusCode::CREATED);

        let all = router
            .clone()
            .oneshot(read_request(READ_KEY, "/api/v1/incidents")?)
            .await?;
        let all_body = json_body(all).await?;
        assert_eq!(all_body["incidents"].as_array().map(Vec::len), Some(2));

        let with_annotation = router
            .clone()
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/incidents?with_annotation=acknowledged",
            )?)
            .await?;
        let with_body = json_body(with_annotation).await?;
        assert_eq!(with_body["incidents"].as_array().map(Vec::len), Some(1));
        assert_eq!(with_body["incidents"][0]["id"], annotated_incident_id);

        let without_annotation = router
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/incidents?without_annotation=acknowledged",
            )?)
            .await?;
        let without_body = json_body(without_annotation).await?;
        assert_eq!(without_body["incidents"].as_array().map(Vec::len), Some(1));
        assert_ne!(without_body["incidents"][0]["id"], annotated_incident_id);

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
    async fn responder_incident_detail_returns_redacted_context_and_audit_event()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let incident_id = open_test_incident(&state).await?;
        let router = ingest_router(state);

        let claim = router
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/claims",
                RESPONDER_KEY,
                &format!(
                    r#"{{
                        "subject_type":"incident",
                        "subject_id":"{incident_id}",
                        "owner":"alice@example.com",
                        "purpose":"follow token=sk_live_claim_secret before paging",
                        "ttl_ms":900000,
                        "idempotency_key":"responder-context-redaction",
                        "evidence_links":["https://ops.example/run?token=sk_live_evidence_secret&user=alice@example.com"]
                    }}"#
                ),
            )?)
            .await?;
        assert_eq!(claim.status(), StatusCode::CREATED);

        let annotation = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/annotations"),
                RESPONDER_KEY,
                r#"{
                    "agent":"alice@example.com",
                    "action":"triaged",
                    "metadata":{
                        "authorization":"Bearer abc.def.ghi",
                        "api_key":"sk_live_nested_secret",
                        "nested":{"email":"bob@example.com","note":"password=\"hunter2\""}
                    }
                }"#,
            )?)
            .await?;
        assert_eq!(annotation.status(), StatusCode::CREATED);

        let detail = router
            .clone()
            .oneshot(read_request(
                RESPONDER_KEY,
                &format!("/api/v1/incidents/{incident_id}"),
            )?)
            .await?;
        let detail_status = detail.status();
        let detail_body = json_body(detail).await?;
        assert_eq!(detail_status, StatusCode::OK, "{detail_body}");

        assert_eq!(
            detail_body["context_envelope"]["schema"],
            "canary.responder_context.incident.v1"
        );
        assert_eq!(
            detail_body["context_envelope"]["tenant_id"],
            canary_store::BOOTSTRAP_TENANT_ID
        );
        assert_eq!(
            detail_body["context_envelope"]["project_id"],
            canary_store::BOOTSTRAP_PROJECT_ID
        );
        assert_eq!(detail_body["context_envelope"]["service"], "test-svc");
        assert_eq!(
            detail_body["context_envelope"]["subject"]["type"],
            "incident"
        );
        assert_eq!(
            detail_body["context_envelope"]["subject"]["id"],
            incident_id
        );
        assert_eq!(
            detail_body["context_envelope"]["retention"]["class"],
            "audit"
        );
        assert_eq!(
            detail_body["context_envelope"]["privacy_policy"]["classification"],
            "redacted"
        );
        assert_eq!(
            detail_body["context_envelope"]["bounds"]["signals"]["max"],
            25
        );
        assert_eq!(
            detail_body["context_envelope"]["bounds"]["annotations"]["max"],
            20
        );
        assert!(
            detail_body["context_envelope"]["audit_event_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("EVT-"))
        );

        let rendered = serde_json::to_string(&detail_body)?;
        for leaked in [
            "sk_live_claim_secret",
            "sk_live_evidence_secret",
            "sk_live_nested_secret",
            "alice@example.com",
            "bob@example.com",
            "abc.def.ghi",
            "hunter2",
        ] {
            assert!(!rendered.contains(leaked), "{leaked} leaked in {rendered}");
        }
        assert_eq!(
            detail_body["annotations"][0]["metadata"]["authorization"],
            "[REDACTED]"
        );
        assert_eq!(
            detail_body["annotations"][0]["metadata"]["api_key"],
            "[REDACTED]"
        );
        assert_eq!(
            detail_body["annotations"][0]["metadata"]["nested"]["email"],
            "[EMAIL]"
        );
        assert_eq!(detail_body["claims"][0]["owner"], "[EMAIL]");
        assert_eq!(
            detail_body["claims"][0]["evidence_links"][0],
            "https://ops.example/run?token=[REDACTED]&user=[EMAIL]"
        );

        let audit = router
            .oneshot(read_request(
                READ_KEY,
                "/api/v1/timeline?service=test-svc&event_type=telemetry.event&limit=5",
            )?)
            .await?;
        let audit_status = audit.status();
        let audit_body = json_body(audit).await?;
        assert_eq!(audit_status, StatusCode::OK, "{audit_body}");
        assert_eq!(audit_body["returned_count"], 1);
        let event = &audit_body["events"][0];
        assert_eq!(event["signal_name"], "responder.context_read");
        assert_eq!(event["retention_class"], "audit");
        assert_eq!(event["privacy_policy"], "redacted");
        assert_eq!(event["attributes"]["reader"]["key_id"], "KEY-responder");
        assert_eq!(event["attributes"]["reader"]["scope"], "responder-write");
        assert_eq!(event["attributes"]["reader"]["service"], "test-svc");
        assert_eq!(event["attributes"]["subject"]["type"], "incident");
        assert_eq!(event["attributes"]["subject"]["id"], incident_id);
        assert_eq!(
            event["attributes"]["context_envelope"]["schema"],
            "canary.responder_context.incident.v1"
        );
        assert!(event["attributes"].get("response_body").is_none());

        Ok(())
    }

    #[tokio::test]
    async fn responder_incident_detail_rejects_cross_service_reads_with_scope_problem()
    -> Result<(), Box<dyn Error>> {
        const API_RESPONDER_KEY: &str = "sk_live_incident_api_responder_secret";
        let state = test_ingest_state()?;
        let incident_id = {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_responder_api_key_for_service(
                &mut store,
                "KEY-incident-api-responder",
                API_RESPONDER_KEY,
                "api",
            )?;
            let error_id = canary_core::ids::ErrorId::generate().into_string();
            let event_id = canary_core::ids::EventId::generate().into_string();
            store.commit_error_ingest(owned_error_ingest(
                canary_store::BOOTSTRAP_TENANT_ID,
                canary_store::BOOTSTRAP_PROJECT_ID,
                &error_id,
                &event_id,
                "group-web-read-deny",
                "web",
                "web incident",
            )?)?;
            let incident_id = canary_core::ids::IncidentId::generate().into_string();
            store.correlate_incident(IncidentCorrelation {
                tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
                project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
                signal_type: "error_group".to_owned(),
                signal_ref: "group-web-read-deny".to_owned(),
                service: "web".to_owned(),
                incident_id: incident_id.parse()?,
                event_id: canary_core::ids::EventId::generate(),
                now: server_time::current_rfc3339(),
            })?;
            incident_id
        };

        let response = ingest_router(state)
            .oneshot(read_request(
                API_RESPONDER_KEY,
                &format!("/api/v1/incidents/{incident_id}"),
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "insufficient_scope");
        assert_eq!(body["bound_service"], "api");
        assert_eq!(body["requested_service"], "web");

        Ok(())
    }

    #[tokio::test]
    async fn read_routes_reject_unbound_responder_write_keys() -> Result<(), Box<dyn Error>> {
        const UNBOUND_RESPONDER_KEY: &str = "sk_live_unbound_responder_secret";
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_api_key(
                &mut store,
                "KEY-unbound-responder",
                UNBOUND_RESPONDER_KEY,
                "responder-write",
                None,
            )?;
        }

        let response = ingest_router(state)
            .oneshot(read_request(UNBOUND_RESPONDER_KEY, "/api/v1/incidents")?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "insufficient_scope");
        assert_eq!(body["scope"], "responder-write");
        assert_eq!(body["required_service_binding"], true);

        Ok(())
    }

    /// Ingest one error and correlate it into an open incident for `test-svc`
    /// (the service RESPONDER_KEY is bound to in `test_ingest_state`).
    async fn open_test_incident(state: &IngestState) -> Result<String, Box<dyn Error>> {
        let router = ingest_router(state.clone());
        let created_error = router
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        let body = json_body(created_error).await?;
        let group_hash = body["group_hash"]
            .as_str()
            .ok_or("missing group hash")?
            .to_owned();
        let incident_id = {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            let id = canary_core::ids::IncidentId::generate().into_string();
            store.correlate_incident(IncidentCorrelation {
                tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
                project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
                signal_type: "error_group".to_owned(),
                signal_ref: group_hash,
                service: "test-svc".to_owned(),
                incident_id: id.parse()?,
                event_id: canary_core::ids::EventId::generate(),
                now: "2026-05-28T20:00:00Z".to_owned(),
            })?;
            id
        };
        Ok(incident_id)
    }

    fn escalate_body(idempotency_key: &str) -> String {
        json!({
            "reason": "hypothesis confidence high, iteration guard exhausted",
            "owner": "bitterblossom/canary-triage",
            "purpose": "triage_escalation",
            "idempotency_key": idempotency_key,
        })
        .to_string()
    }

    #[tokio::test]
    async fn escalate_incident_requires_responder_write_scope() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let incident_id = open_test_incident(&state).await?;

        let response = ingest_router(state)
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/escalate"),
                READ_KEY,
                &escalate_body("bb-run-1:escalate"),
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "insufficient_scope");

        Ok(())
    }

    #[tokio::test]
    async fn escalate_incident_sets_escalation_and_enqueues_webhook() -> Result<(), Box<dyn Error>>
    {
        let sink = Arc::new(RecordingFailingSink::default());
        let state = test_ingest_state_with_sink(sink.clone())?;
        let incident_id = open_test_incident(&state).await?;

        let response = ingest_router(state)
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/escalate"),
                RESPONDER_KEY,
                &escalate_body("bb-run-1:escalate"),
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["escalation"]["incident_id"], incident_id);
        assert_eq!(
            body["escalation"]["escalated_by"],
            "bitterblossom/canary-triage"
        );
        assert!(body["escalation"]["escalated_at"].is_string());
        assert_eq!(
            body["escalation"]["reason"],
            "hypothesis confidence high, iteration guard exhausted"
        );

        let effects = sink.effects.lock().map_err(|_| "effect lock poisoned")?;
        let webhook = effects
            .iter()
            .find_map(|effect| match effect {
                IngestEffect::EnqueueWebhook {
                    event,
                    payload_json,
                } if event == "incident.escalated" => Some(payload_json),
                _ => None,
            })
            .ok_or("expected incident.escalated webhook effect")?;
        let payload: Value = serde_json::from_str(webhook)?;
        assert_eq!(payload["event"], "incident.escalated");
        assert_eq!(payload["service"], "test-svc");
        assert_eq!(payload["escalation"]["incident_id"], incident_id);
        assert_eq!(
            payload["escalation"]["escalated_by"],
            "bitterblossom/canary-triage"
        );

        Ok(())
    }

    #[tokio::test]
    async fn escalate_incident_is_idempotent_by_key_over_http() -> Result<(), Box<dyn Error>> {
        let sink = Arc::new(RecordingFailingSink::default());
        let state = test_ingest_state_with_sink(sink.clone())?;
        let incident_id = open_test_incident(&state).await?;
        let router = ingest_router(state);

        let first = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/escalate"),
                RESPONDER_KEY,
                &escalate_body("bb-run-1:escalate"),
            )?)
            .await?;
        assert_eq!(first.status(), StatusCode::CREATED);
        let first_body = json_body(first).await?;

        let replay = router
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/escalate"),
                RESPONDER_KEY,
                &escalate_body("bb-run-1:escalate"),
            )?)
            .await?;
        assert_eq!(replay.status(), StatusCode::OK);
        let replay_body = json_body(replay).await?;
        assert_eq!(replay_body["escalation"], first_body["escalation"]);

        let escalated_webhooks = sink
            .effects
            .lock()
            .map_err(|_| "effect lock poisoned")?
            .iter()
            .filter(|effect| {
                matches!(effect, IngestEffect::EnqueueWebhook { event, .. } if event == "incident.escalated")
            })
            .count();
        assert_eq!(
            escalated_webhooks, 1,
            "replay must not re-enqueue a webhook"
        );

        Ok(())
    }

    #[tokio::test]
    async fn escalate_incident_rejects_already_resolved_incident() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let incident_id = open_test_incident(&state).await?;
        {
            // The attached error_group signal's `last_seen_at` was stamped
            // at real ingest time. Correlating far enough in the future
            // pushes it outside the 300s active window, which resolves the
            // incident (and, in the same transaction, would clear any
            // escalation — exercised separately at the store layer).
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            store.correlate_incident(IncidentCorrelation {
                tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
                project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
                signal_type: "error_group".to_owned(),
                signal_ref: "resolved-by-test".to_owned(),
                service: "test-svc".to_owned(),
                incident_id: canary_core::ids::IncidentId::generate(),
                event_id: canary_core::ids::EventId::generate(),
                now: "2099-01-01T00:00:00Z".to_owned(),
            })?;
        }

        let response = ingest_router(state)
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/escalate"),
                RESPONDER_KEY,
                &escalate_body("bb-run-1:escalate"),
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "incident_already_resolved");

        Ok(())
    }

    #[tokio::test]
    async fn escalate_incident_returns_not_found_for_missing_incident() -> Result<(), Box<dyn Error>>
    {
        let response = ingest_router(test_ingest_state()?)
            .oneshot(json_request(
                "POST",
                "/api/v1/incidents/INC-missing/escalate",
                ADMIN_KEY,
                &escalate_body("bb-run-1:escalate"),
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["detail"], "Incident INC-missing not found.");

        Ok(())
    }

    #[tokio::test]
    async fn escalate_incident_validates_required_fields() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let incident_id = open_test_incident(&state).await?;

        let response = ingest_router(state)
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/escalate"),
                RESPONDER_KEY,
                r#"{"owner":"bitterblossom/canary-triage"}"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body["errors"]["reason"],
            json!(["must be a non-empty string"])
        );
        assert_eq!(
            body["errors"]["purpose"],
            json!(["must be a non-empty string"])
        );
        assert_eq!(
            body["errors"]["idempotency_key"],
            json!(["must be a non-empty string"])
        );

        Ok(())
    }

    #[tokio::test]
    async fn deescalate_incident_clears_escalation_and_enqueues_webhook()
    -> Result<(), Box<dyn Error>> {
        let sink = Arc::new(RecordingFailingSink::default());
        let state = test_ingest_state_with_sink(sink.clone())?;
        let incident_id = open_test_incident(&state).await?;
        let router = ingest_router(state);

        let escalated = router
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/escalate"),
                RESPONDER_KEY,
                &escalate_body("bb-run-1:escalate"),
            )?)
            .await?;
        assert_eq!(escalated.status(), StatusCode::CREATED);

        let response = router
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/deescalate"),
                RESPONDER_KEY,
                r#"{"owner":"operator@example.com","reason":"false positive"}"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["escalation"]["incident_id"], incident_id);
        assert_eq!(body["escalation"]["escalated_at"], Value::Null);
        assert_eq!(body["escalation"]["escalated_by"], Value::Null);
        assert_eq!(body["escalation"]["reason"], Value::Null);

        let effects = sink.effects.lock().map_err(|_| "effect lock poisoned")?;
        let webhook = effects
            .iter()
            .find_map(|effect| match effect {
                IngestEffect::EnqueueWebhook {
                    event,
                    payload_json,
                } if event == "incident.deescalated" => Some(payload_json),
                _ => None,
            })
            .ok_or("expected incident.deescalated webhook effect")?;
        let payload: Value = serde_json::from_str(webhook)?;
        assert_eq!(payload["event"], "incident.deescalated");
        assert_eq!(payload["escalation"]["incident_id"], incident_id);
        assert_eq!(payload["escalation"]["escalated_at"], Value::Null);

        Ok(())
    }

    #[tokio::test]
    async fn deescalate_incident_requires_responder_write_scope() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let incident_id = open_test_incident(&state).await?;

        let response = ingest_router(state)
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/incidents/{incident_id}/deescalate"),
                READ_KEY,
                r#"{"owner":"operator@example.com"}"#,
            )?)
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
    async fn error_ingest_rejects_bad_persisted_scope_and_accounts_auth_fail()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?.with_auth_fail_identity(AuthFailIdentityConfig {
            trust_proxy_headers: true,
        });
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            seed_api_key(
                &mut store,
                "KEY-bad-scope",
                "sk_live_bad_scope_secret",
                "super-admin",
                None,
            )?;
        }
        let router = ingest_router(state.clone());

        let response = router
            .oneshot(
                Request::post("/api/v1/errors")
                    .header("authorization", "Bearer sk_live_bad_scope_secret")
                    .header("fly-client-ip", "203.0.113.12")
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from(valid_error_body()))?,
            )
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "invalid_api_key");

        let mut limiter = state
            .rate_limiter()
            .lock()
            .map_err(|_| "rate limiter lock poisoned")?;
        for _ in 0..9 {
            assert_eq!(
                limiter.check(RateLimitKind::AuthFail, "203.0.113.12"),
                RateLimitDecision::Allowed
            );
        }
        assert!(matches!(
            limiter.check(RateLimitKind::AuthFail, "203.0.113.12"),
            RateLimitDecision::Limited { .. }
        ));

        Ok(())
    }

    #[tokio::test]
    async fn service_bound_ingest_key_cannot_impersonate_another_service()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let raw_key = "sk_live_linejam_bound_secret";
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            store.insert_api_key(ApiKeyInsert {
                id: "KEY-linejam-bound".to_owned(),
                name: "linejam browser ingest".to_owned(),
                key_prefix: raw_key.chars().take(API_KEY_PREFIX_LEN).collect(),
                key_hash: bcrypt::hash(raw_key, TEST_BCRYPT_COST)?,
                created_at: "2026-05-28T20:00:00Z".to_owned(),
                revoked_at: None,
                scope: "ingest-only".to_owned(),
                tenant_id: "TENANT-alpha".to_owned(),
                project_id: "PROJECT-web".to_owned(),
                service: Some("linejam".to_owned()),
            })?;
        }

        let response = ingest_router(state.clone())
            .oneshot(error_request(
                raw_key,
                r#"{"service":"vanity","error_class":"RuntimeError","message":"spoofed"}"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "insufficient_scope");
        assert_eq!(body["bound_service"], "linejam");
        assert_eq!(body["requested_service"], "vanity");
        assert_eq!(error_count(&state)?, 0);

        Ok(())
    }

    #[tokio::test]
    async fn telemetry_event_webhooks_keep_caller_owner_and_service_scope()
    -> Result<(), Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        store.insert_api_key(ApiKeyInsert {
            id: "KEY-linejam-ingest".to_owned(),
            name: "linejam ingest".to_owned(),
            key_prefix: OTHER_INGEST_KEY.chars().take(API_KEY_PREFIX_LEN).collect(),
            key_hash: bcrypt::hash(OTHER_INGEST_KEY, TEST_BCRYPT_COST)?,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
            revoked_at: None,
            scope: "ingest-only".to_owned(),
            tenant_id: "TENANT-alpha".to_owned(),
            project_id: "PROJECT-web".to_owned(),
            service: Some("linejam".to_owned()),
        })?;
        let mut matching = webhook_subscription_insert(
            "WHK-linejam",
            "https://example.test/linejam",
            vec!["telemetry.event".to_owned()],
            "test-webhook-secret",
            true,
            "2026-05-28T20:00:00Z",
        );
        matching.tenant_id = "TENANT-alpha".to_owned();
        matching.project_id = "PROJECT-web".to_owned();
        matching.service = Some("linejam".to_owned());
        store.insert_webhook_subscription(matching)?;
        store.insert_webhook_subscription(webhook_subscription_insert(
            "WHK-bootstrap",
            "https://example.test/bootstrap",
            vec!["telemetry.event".to_owned()],
            "test-webhook-secret",
            true,
            "2026-05-28T20:00:00Z",
        ))?;
        let scheduler = Arc::new(RecordingScheduler::default());
        let state = IngestState::new_with_webhook_scheduler(
            store,
            IngestConfig::default(),
            scheduler.clone(),
        );

        let response = ingest_router(state.clone())
            .oneshot(json_request(
                "POST",
                "/api/v1/events",
                OTHER_INGEST_KEY,
                r#"{"service":"linejam","name":"agent.workflow.completed","summary":"done"}"#,
            )?)
            .await?;

        assert_eq!(response.status(), StatusCode::CREATED);
        let jobs = scheduler.jobs()?;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].webhook_id, "WHK-linejam");
        assert_eq!(jobs[0].event, "telemetry.event");
        assert_eq!(jobs[0].payload["tenant_id"], "TENANT-alpha");
        assert_eq!(jobs[0].payload["project_id"], "PROJECT-web");
        assert_eq!(jobs[0].payload["service"], "linejam");

        let store = state.lock_store().map_err(|_| "store lock poisoned")?;
        let deliveries = store.webhook_deliveries(canary_store::WebhookDeliveryListOptions {
            event: Some("telemetry.event".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].webhook_id, "WHK-linejam");
        assert_eq!(deliveries[0].tenant_id, "TENANT-alpha");
        assert_eq!(deliveries[0].project_id, "PROJECT-web");
        assert_eq!(deliveries[0].service.as_deref(), Some("linejam"));

        Ok(())
    }

    #[tokio::test]
    async fn service_bound_ingest_key_cannot_emit_events_for_another_service()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        let raw_key = "sk_live_linejam_event_bound_secret";
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            store.insert_api_key(ApiKeyInsert {
                id: "KEY-linejam-event-bound".to_owned(),
                name: "linejam event ingest".to_owned(),
                key_prefix: raw_key.chars().take(API_KEY_PREFIX_LEN).collect(),
                key_hash: bcrypt::hash(raw_key, TEST_BCRYPT_COST)?,
                created_at: "2026-05-28T20:00:00Z".to_owned(),
                revoked_at: None,
                scope: "ingest-only".to_owned(),
                tenant_id: "TENANT-alpha".to_owned(),
                project_id: "PROJECT-web".to_owned(),
                service: Some("linejam".to_owned()),
            })?;
        }

        let response = ingest_router(state.clone())
            .oneshot(json_request(
                "POST",
                "/api/v1/events",
                raw_key,
                r#"{"service":"vanity","name":"signup.completed","summary":"spoofed"}"#,
            )?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "insufficient_scope");
        assert_eq!(body["bound_service"], "linejam");
        assert_eq!(body["requested_service"], "vanity");

        Ok(())
    }

    #[tokio::test]
    async fn error_ingest_wrong_scope_does_not_account_auth_fail() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?.with_auth_fail_identity(AuthFailIdentityConfig {
            trust_proxy_headers: true,
        });
        let router = ingest_router(state.clone());

        for _ in 0..10 {
            let response = router
                .clone()
                .oneshot(
                    Request::post("/api/v1/errors")
                        .header("authorization", format!("Bearer {READ_KEY}"))
                        .header("fly-client-ip", "203.0.113.13")
                        .header(CONTENT_TYPE, APPLICATION_JSON)
                        .body(Body::from(valid_error_body()))?,
                )
                .await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(body["code"], "insufficient_scope");
        }

        let mut limiter = state
            .rate_limiter()
            .lock()
            .map_err(|_| "rate limiter lock poisoned")?;
        assert_eq!(
            limiter.check(RateLimitKind::AuthFail, "203.0.113.13"),
            RateLimitDecision::Allowed
        );

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
    async fn ingest_and_query_routes_enforce_rate_limit_buckets() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut limiter = state
                .rate_limiter()
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

    #[tokio::test]
    async fn ingest_rate_limit_uses_durable_store_bucket_after_auth() -> Result<(), Box<dyn Error>>
    {
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
            let now_ms = crate::server_time::current_unix_millis();
            for offset in 0..100 {
                assert_eq!(
                    store.check_rate_limit("ingest", "KEY-ingest", 100, 60_000, now_ms + offset)?,
                    canary_store::DurableRateLimitDecision::Allowed
                );
            }
        }

        let response = ingest_router(state)
            .oneshot(error_request(INGEST_KEY, valid_error_body())?)
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], "rate_limited");
        assert!(
            body["retry_after"]
                .as_u64()
                .is_some_and(|retry_after| (1..=60).contains(&retry_after))
        );

        Ok(())
    }

    #[tokio::test]
    async fn invalid_api_keys_are_silently_accounted_by_proxy_identity()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?.with_auth_fail_identity(AuthFailIdentityConfig {
            trust_proxy_headers: true,
        });
        let router = ingest_router(state.clone());

        let response = router
            .clone()
            .oneshot(
                Request::post("/api/v1/errors")
                    .header("authorization", "Bearer sk_live_unknown_secret")
                    .header("fly-client-ip", "203.0.113.9")
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from(valid_error_body()))?,
            )
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "invalid_api_key");

        {
            let mut limiter = state
                .rate_limiter()
                .lock()
                .map_err(|_| "rate limiter lock poisoned")?;
            for _ in 0..9 {
                assert_eq!(
                    limiter.check(RateLimitKind::AuthFail, "203.0.113.9"),
                    RateLimitDecision::Allowed
                );
            }
            assert!(matches!(
                limiter.check(RateLimitKind::AuthFail, "203.0.113.9"),
                RateLimitDecision::Limited { .. }
            ));
        }

        let response = router
            .oneshot(
                Request::post("/api/v1/errors")
                    .header("authorization", format!("Bearer {INGEST_KEY}"))
                    .header("fly-client-ip", "203.0.113.9")
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from(valid_error_body()))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::CREATED);

        Ok(())
    }

    #[tokio::test]
    async fn invalid_api_keys_are_rejected_by_auth_fail_rate_limit() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?.with_auth_fail_identity(AuthFailIdentityConfig {
            trust_proxy_headers: true,
        });
        let router = ingest_router(state);

        for _ in 0..10 {
            let response = router
                .clone()
                .oneshot(
                    Request::post("/api/v1/errors")
                        .header("authorization", format!("Bearer {WRONG_INGEST_PREFIX_KEY}"))
                        .header("fly-client-ip", "203.0.113.9")
                        .header(CONTENT_TYPE, APPLICATION_JSON)
                        .body(Body::from(valid_error_body()))?,
                )
                .await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body["code"], "invalid_api_key");
        }

        let response = router
            .oneshot(
                Request::post("/api/v1/errors")
                    .header("authorization", format!("Bearer {WRONG_INGEST_PREFIX_KEY}"))
                    .header("fly-client-ip", "203.0.113.9")
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from(valid_error_body()))?,
            )
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], "rate_limited");
        assert!(body["retry_after"].as_u64().is_some());

        Ok(())
    }

    #[tokio::test]
    async fn default_auth_fail_identity_ignores_spoofed_proxy_headers() -> Result<(), Box<dyn Error>>
    {
        let state = test_ingest_state()?;
        let router = ingest_router(state.clone());

        let response = router
            .clone()
            .oneshot(
                Request::post("/api/v1/errors")
                    .header("authorization", "Bearer sk_live_unknown_secret")
                    .header("x-forwarded-for", "198.51.100.4")
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from(valid_error_body()))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let mut limiter = state
            .rate_limiter()
            .lock()
            .map_err(|_| "rate limiter lock poisoned")?;
        assert_eq!(
            limiter.check(RateLimitKind::AuthFail, "198.51.100.4"),
            RateLimitDecision::Allowed
        );
        for _ in 0..9 {
            assert_eq!(
                limiter.check(RateLimitKind::AuthFail, UNKNOWN_AUTH_FAIL_IDENTITY),
                RateLimitDecision::Allowed
            );
        }
        assert!(matches!(
            limiter.check(RateLimitKind::AuthFail, UNKNOWN_AUTH_FAIL_IDENTITY),
            RateLimitDecision::Limited { .. }
        ));

        Ok(())
    }

    #[tokio::test]
    async fn missing_authorization_does_not_account_auth_fail() -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?.with_auth_fail_identity(AuthFailIdentityConfig {
            trust_proxy_headers: true,
        });
        let router = ingest_router(state.clone());

        for _ in 0..20 {
            let response = router
                .clone()
                .oneshot(
                    Request::post("/api/v1/errors")
                        .header("fly-client-ip", "203.0.113.10")
                        .header(CONTENT_TYPE, APPLICATION_JSON)
                        .body(Body::from(valid_error_body()))?,
                )
                .await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body["code"], "invalid_api_key");
        }

        let mut limiter = state
            .rate_limiter()
            .lock()
            .map_err(|_| "rate limiter lock poisoned")?;
        assert_eq!(
            limiter.check(RateLimitKind::AuthFail, "203.0.113.10"),
            RateLimitDecision::Allowed
        );

        Ok(())
    }

    #[test]
    fn auth_fail_identity_parses_trusted_proxy_headers_in_priority_order()
    -> Result<(), Box<dyn Error>> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.4, 203.0.113.11"),
        );
        headers.insert(
            "forwarded",
            HeaderValue::from_static("for=198.51.100.8;proto=https, for=192.0.2.7"),
        );
        headers.insert("fly-client-ip", HeaderValue::from_static("203.0.113.9"));

        assert_eq!(
            auth_fail_identity(
                &headers,
                AuthFailIdentityConfig {
                    trust_proxy_headers: true
                }
            ),
            "203.0.113.9"
        );

        headers.remove("fly-client-ip");
        assert_eq!(
            auth_fail_identity(
                &headers,
                AuthFailIdentityConfig {
                    trust_proxy_headers: true
                }
            ),
            "192.0.2.7"
        );

        headers.remove("forwarded");
        assert_eq!(
            auth_fail_identity(
                &headers,
                AuthFailIdentityConfig {
                    trust_proxy_headers: true
                }
            ),
            "203.0.113.11"
        );

        Ok(())
    }

    #[tokio::test]
    async fn auth_fail_identity_canonicalizes_proxy_ports_for_accounting()
    -> Result<(), Box<dyn Error>> {
        let state = test_ingest_state()?.with_auth_fail_identity(AuthFailIdentityConfig {
            trust_proxy_headers: true,
        });
        let router = ingest_router(state.clone());

        let response = router
            .clone()
            .oneshot(
                Request::post("/api/v1/errors")
                    .header("authorization", "Bearer sk_live_unknown_secret")
                    .header("forwarded", r#"for="[2001:db8::1]:40000""#)
                    .header("x-forwarded-for", "198.51.100.4:40000")
                    .header(CONTENT_TYPE, APPLICATION_JSON)
                    .body(Body::from(valid_error_body()))?,
            )
            .await?;
        let status = response.status();
        let body = json_body(response).await?;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "invalid_api_key");

        let mut limiter = state
            .rate_limiter()
            .lock()
            .map_err(|_| "rate limiter lock poisoned")?;
        for _ in 0..9 {
            assert_eq!(
                limiter.check(RateLimitKind::AuthFail, "2001:db8::1"),
                RateLimitDecision::Allowed
            );
        }
        assert!(matches!(
            limiter.check(RateLimitKind::AuthFail, "2001:db8::1"),
            RateLimitDecision::Limited { .. }
        ));
        assert_eq!(
            limiter.check(RateLimitKind::AuthFail, "[2001:db8::1]:40000"),
            RateLimitDecision::Allowed
        );

        Ok(())
    }

    async fn json_body(response: Response<Body>) -> Result<Value, Box<dyn Error>> {
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice(&bytes)?;

        Ok(body)
    }

    async fn assert_problem_details(
        response: Response<Body>,
        status: StatusCode,
        code: &str,
    ) -> Result<(), Box<dyn Error>> {
        assert_eq!(response.status(), status);
        assert_eq!(
            response.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static(
                http_contract::PROBLEM_CONTENT_TYPE
            ))
        );
        let body = json_body(response).await?;
        assert_eq!(body["status"], status.as_u16());
        assert_eq!(body["code"], code);
        for field in ["type", "title", "detail"] {
            assert!(
                body[field].as_str().is_some_and(|value| !value.is_empty()),
                "missing non-empty Problem Details field: {field}"
            );
        }
        if let Some(request_id) = body.get("request_id").filter(|value| !value.is_null()) {
            assert!(request_id.as_str().is_some());
        }
        Ok(())
    }

    async fn text_body(response: Response<Body>) -> Result<String, Box<dyn Error>> {
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        Ok(String::from_utf8(bytes.to_vec())?)
    }

    type OpenApiOperation<'a> = (String, String, &'a Value);

    fn openapi_operations(document: &Value) -> Result<Vec<OpenApiOperation<'_>>, Box<dyn Error>> {
        let paths = document["paths"]
            .as_object()
            .ok_or("OpenAPI paths must be an object")?;
        let mut operations = Vec::new();

        for (path, path_item) in paths {
            let path_item = path_item
                .as_object()
                .ok_or("OpenAPI path item must be an object")?;
            for (method, operation) in path_item {
                if !matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete") {
                    continue;
                }
                operations.push((method.to_uppercase(), path.clone(), operation));
            }
        }

        Ok(operations)
    }

    fn openapi_authenticated_operations(
        document: &Value,
    ) -> Result<std::collections::BTreeSet<(String, String)>, Box<dyn Error>> {
        Ok(openapi_operations(document)?
            .into_iter()
            .filter(|(_, _, operation)| !openapi_operation_is_public(operation))
            .map(|(method, path, _)| (method, path))
            .collect())
    }

    fn openapi_operation<'a>(
        document: &'a Value,
        path: &str,
        method: &str,
    ) -> Result<&'a Value, Box<dyn Error>> {
        document
            .pointer(&format!(
                "/paths/{}/{}",
                escape_json_pointer(path),
                method.to_lowercase()
            ))
            .ok_or_else(|| format!("missing OpenAPI operation {method} {path}").into())
    }

    fn openapi_operation_is_public(operation: &Value) -> bool {
        operation
            .get("security")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    }

    fn route_operations_from_source(
        source: &str,
    ) -> Result<std::collections::BTreeSet<(String, String)>, Box<dyn Error>> {
        let mut operations = std::collections::BTreeSet::new();
        let mut offset = 0;

        while let Some(relative_start) = source[offset..].find(".route(") {
            let start = offset + relative_start;
            let next_route = source[start + 1..]
                .find(".route(")
                .map(|relative| start + 1 + relative);
            let next_state = source[start..]
                .find(".with_state(")
                .map(|relative| start + relative);
            let end = match (next_route, next_state) {
                (Some(route), Some(state)) => route.min(state),
                (Some(route), None) => route,
                (None, Some(state)) => state,
                (None, None) => source.len(),
            };
            let route_call = &source[start..end];
            let path = route_call
                .split_once('"')
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(path, _)| path)
                .ok_or("route call missing path literal")?;

            for (needle, method) in [
                ("delete(", "DELETE"),
                ("get(", "GET"),
                ("patch(", "PATCH"),
                ("post(", "POST"),
                ("put(", "PUT"),
            ] {
                if route_call.contains(needle) {
                    operations.insert((method.to_owned(), path.to_owned()));
                }
            }

            offset = end;
        }

        Ok(operations)
    }

    fn json_response_schema<'a>(
        document: &'a Value,
        response: &'a Value,
    ) -> Result<Option<&'a Value>, Box<dyn Error>> {
        let response = match response.get("$ref").and_then(Value::as_str) {
            Some(reference) => resolve_openapi_ref(document, reference)
                .ok_or_else(|| format!("unresolved response ref: {reference}"))?,
            None => response,
        };

        Ok(["application/json", "application/problem+json"]
            .iter()
            .find_map(|content_type| {
                response
                    .pointer(&format!(
                        "/content/{}/schema",
                        escape_json_pointer(content_type)
                    ))
                    .filter(|schema| schema.is_object())
            }))
    }

    struct SummaryExceptions<'a> {
        schemas: std::collections::BTreeSet<&'a str>,
        operations: std::collections::BTreeSet<(String, String)>,
    }

    fn openapi_summary_exceptions(
        document: &Value,
    ) -> Result<SummaryExceptions<'_>, Box<dyn Error>> {
        let entries = document
            .pointer("/info/x-agent-guide/summary_exceptions")
            .and_then(Value::as_array)
            .ok_or("missing info.x-agent-guide.summary_exceptions")?;
        let mut schemas = std::collections::BTreeSet::new();
        let mut operations = std::collections::BTreeSet::new();

        for entry in entries {
            let reason = entry
                .get("reason")
                .and_then(Value::as_str)
                .ok_or("summary exception entry missing reason")?;
            assert!(!reason.is_empty(), "summary exception reason is empty");
            for schema in entry
                .get("schemas")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let schema = schema
                    .as_str()
                    .ok_or("summary exception schema must be a string")?;
                schemas.insert(schema);
            }
            for operation in entry
                .get("operations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let operation = operation
                    .as_object()
                    .ok_or("summary exception operation must be an object")?;
                let method = operation
                    .get("method")
                    .and_then(Value::as_str)
                    .ok_or("summary exception operation missing method")?;
                let path = operation
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or("summary exception operation missing path")?;
                operations.insert((method.to_owned(), path.to_owned()));
            }
        }

        assert!(
            !schemas.is_empty() || !operations.is_empty(),
            "summary exception table must list at least one schema or operation"
        );
        let component_schemas = document
            .pointer("/components/schemas")
            .and_then(Value::as_object)
            .ok_or("missing components.schemas")?;
        for schema in &schemas {
            assert!(
                component_schemas.contains_key(*schema),
                "summary exception references missing schema: {schema}"
            );
        }
        let operation_set = openapi_operations(document)?
            .into_iter()
            .map(|(method, path, _)| (method, path))
            .collect::<std::collections::BTreeSet<_>>();
        for operation in &operations {
            assert!(
                operation_set.contains(operation),
                "summary exception references missing operation: {} {}",
                operation.0,
                operation.1
            );
        }

        Ok(SummaryExceptions {
            schemas,
            operations,
        })
    }

    fn schema_has_summary(document: &Value, schema: &Value, seen: &mut Vec<String>) -> bool {
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            if seen.iter().any(|seen| seen == reference) {
                return false;
            }
            let Some(resolved) = resolve_openapi_ref(document, reference) else {
                return false;
            };
            seen.push(reference.to_owned());
            let has_summary = schema_has_summary(document, resolved, seen);
            seen.pop();
            return has_summary;
        }

        if schema_has_required_string_summary(schema) {
            return true;
        }

        if schema
            .get("allOf")
            .and_then(Value::as_array)
            .is_some_and(|schemas| {
                !schemas.is_empty()
                    && schemas
                        .iter()
                        .any(|schema| schema_has_summary(document, schema, seen))
            })
        {
            return true;
        }

        for keyword in ["anyOf", "oneOf"] {
            if let Some(schemas) = schema.get(keyword).and_then(Value::as_array) {
                return !schemas.is_empty()
                    && schemas
                        .iter()
                        .all(|schema| schema_has_summary(document, schema, seen));
            }
        }

        schema
            .get("items")
            .is_some_and(|items| schema_has_summary(document, items, seen))
    }

    fn schema_has_required_string_summary(schema: &Value) -> bool {
        let summary = schema
            .get("properties")
            .and_then(|properties| properties.get("summary"));
        let required = schema.get("required").and_then(Value::as_array);

        summary.is_some_and(|summary| summary.get("type").and_then(Value::as_str) == Some("string"))
            && required.is_some_and(|required| {
                required
                    .iter()
                    .any(|field| field.as_str() == Some("summary"))
            })
    }

    fn schema_required_field(schema: &Value, name: &str) -> bool {
        schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| required.iter().any(|field| field.as_str() == Some(name)))
    }

    fn resolve_openapi_ref<'a>(document: &'a Value, reference: &str) -> Option<&'a Value> {
        document.pointer(reference.strip_prefix('#')?)
    }

    fn schema_ref_name(schema: &Value) -> Option<&str> {
        schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| reference.rsplit('/').next())
    }

    fn escape_json_pointer(value: &str) -> String {
        value.replace('~', "~0").replace('/', "~1")
    }

    fn test_ingest_state() -> Result<IngestState, Box<dyn Error>> {
        test_ingest_state_with_sink(Arc::new(TestNoopIngestEffectSink))
    }

    fn test_ingest_state_with_monitor(name: &str) -> Result<IngestState, Box<dyn Error>> {
        let state = test_ingest_state()?;
        {
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
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
        store.insert_webhook_subscription(webhook_subscription_insert(
            "WHK-monitor",
            "https://example.test/monitor",
            vec![event.to_owned()],
            "test-webhook-secret",
            true,
            "2026-05-28T20:00:00Z",
        ))?;

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
        seed_responder_api_key_for_service(&mut store, "KEY-responder", RESPONDER_KEY, "test-svc")?;
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
        store.insert_webhook_subscription(webhook_subscription_insert(
            "WHK-test",
            "https://example.test/hook",
            vec!["error.new_class".to_owned()],
            "test-webhook-secret",
            active_webhook,
            "2026-05-28T20:00:00Z",
        ))?;

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

    fn wait_for_error_count(path: &Path, expected: u64) -> Result<(), Box<dyn Error>> {
        let deadline = Instant::now() + StdDuration::from_secs(3);
        loop {
            let store = Store::open(path)?;
            if store.error_count()? == expected {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("timed out waiting for {expected} errors").into());
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
        let store = state.lock_store().map_err(|_| "store lock poisoned")?;
        Ok(store.error_count()?)
    }

    fn seed_api_key(
        store: &mut Store,
        id: &str,
        raw_key: &str,
        scope: &str,
        revoked_at: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        seed_api_key_with_owner(
            store,
            id,
            raw_key,
            scope,
            revoked_at,
            canary_store::BOOTSTRAP_TENANT_ID,
            canary_store::BOOTSTRAP_PROJECT_ID,
        )
    }

    fn seed_api_key_with_owner(
        store: &mut Store,
        id: &str,
        raw_key: &str,
        scope: &str,
        revoked_at: Option<&str>,
        tenant_id: &str,
        project_id: &str,
    ) -> Result<(), Box<dyn Error>> {
        store.insert_api_key(ApiKeyInsert {
            id: id.to_owned(),
            name: format!("key {id}"),
            key_prefix: raw_key.chars().take(API_KEY_PREFIX_LEN).collect(),
            key_hash: bcrypt::hash(raw_key, TEST_BCRYPT_COST)?,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
            revoked_at: revoked_at.map(str::to_owned),
            scope: scope.to_owned(),
            tenant_id: tenant_id.to_owned(),
            project_id: project_id.to_owned(),
            service: None,
        })?;
        Ok(())
    }

    fn seed_read_api_key_for_service(
        store: &mut Store,
        id: &str,
        raw_key: &str,
        service: &str,
    ) -> Result<(), Box<dyn Error>> {
        store.insert_api_key(ApiKeyInsert {
            id: id.to_owned(),
            name: format!("key {id}"),
            key_prefix: raw_key.chars().take(API_KEY_PREFIX_LEN).collect(),
            key_hash: bcrypt::hash(raw_key, TEST_BCRYPT_COST)?,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
            revoked_at: None,
            scope: "read-only".to_owned(),
            tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
            project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
            service: Some(service.to_owned()),
        })?;
        Ok(())
    }

    fn seed_responder_api_key_for_service(
        store: &mut Store,
        id: &str,
        raw_key: &str,
        service: &str,
    ) -> Result<(), Box<dyn Error>> {
        store.insert_api_key(ApiKeyInsert {
            id: id.to_owned(),
            name: format!("key {id}"),
            key_prefix: raw_key.chars().take(API_KEY_PREFIX_LEN).collect(),
            key_hash: bcrypt::hash(raw_key, TEST_BCRYPT_COST)?,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
            revoked_at: None,
            scope: "responder-write".to_owned(),
            tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
            project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
            service: Some(service.to_owned()),
        })?;
        Ok(())
    }

    fn seed_admin_api_key_for_service(
        store: &mut Store,
        id: &str,
        raw_key: &str,
        service: &str,
    ) -> Result<(), Box<dyn Error>> {
        store.insert_api_key(ApiKeyInsert {
            id: id.to_owned(),
            name: format!("key {id}"),
            key_prefix: raw_key.chars().take(API_KEY_PREFIX_LEN).collect(),
            key_hash: bcrypt::hash(raw_key, TEST_BCRYPT_COST)?,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
            revoked_at: None,
            scope: "admin".to_owned(),
            tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
            project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
            service: Some(service.to_owned()),
        })?;
        Ok(())
    }

    fn webhook_subscription_insert(
        id: &str,
        url: &str,
        events: Vec<String>,
        secret: &str,
        active: bool,
        created_at: &str,
    ) -> WebhookSubscriptionInsert {
        WebhookSubscriptionInsert {
            id: id.to_owned(),
            tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
            project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
            service: None,
            url: url.to_owned(),
            events,
            secret: secret.to_owned(),
            active,
            created_at: created_at.to_owned(),
        }
    }

    fn webhook_delivery_insert(
        delivery_id: &str,
        webhook_id: &str,
        event: &str,
        now: &str,
    ) -> WebhookDeliveryInsert {
        webhook_delivery_insert_with_owner(
            delivery_id,
            webhook_id,
            event,
            now,
            canary_store::BOOTSTRAP_TENANT_ID,
            canary_store::BOOTSTRAP_PROJECT_ID,
            None,
        )
    }

    fn webhook_delivery_insert_with_owner(
        delivery_id: &str,
        webhook_id: &str,
        event: &str,
        now: &str,
        tenant_id: &str,
        project_id: &str,
        service: Option<&str>,
    ) -> WebhookDeliveryInsert {
        WebhookDeliveryInsert {
            delivery_id: delivery_id.to_owned(),
            webhook_id: webhook_id.to_owned(),
            tenant_id: tenant_id.to_owned(),
            project_id: project_id.to_owned(),
            service: service.map(str::to_owned),
            event: event.to_owned(),
            now: now.to_owned(),
        }
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
        store.insert_target(target_insert(service))?;
        Ok(())
    }

    fn target_insert(service: &str) -> TargetInsert {
        TargetInsert {
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
        }
    }

    fn test_error_ingest(index: usize, created_at: &str) -> ErrorIngest {
        ErrorIngest {
            ids: ErrorIngestIds {
                error_id: ErrorId::generate(),
                event_id: EventId::generate(),
            },
            payload: ErrorIngestPayload {
                tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
                project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
                service: "retention".to_owned(),
                error_class: "RuntimeError".to_owned(),
                message: "old".to_owned(),
                message_template: "old".to_owned(),
                stack_trace: None,
                context_json: None,
                severity: "error".to_owned(),
                environment: "production".to_owned(),
                group_hash: format!("grp-retention-{index}"),
                fingerprint_json: None,
                region: None,
                classification: Classification {
                    category: Category::Application,
                    persistence: Persistence::Persistent,
                    component: Component::Runtime,
                },
                created_at: created_at.to_owned(),
            },
        }
    }

    fn owned_error_ingest(
        tenant_id: &str,
        project_id: &str,
        error_id: &str,
        event_id: &str,
        group_hash: &str,
        service: &str,
        message: &str,
    ) -> Result<ErrorIngest, Box<dyn Error>> {
        Ok(ErrorIngest {
            ids: ErrorIngestIds {
                error_id: error_id.parse()?,
                event_id: event_id.parse()?,
            },
            payload: ErrorIngestPayload {
                tenant_id: tenant_id.to_owned(),
                project_id: project_id.to_owned(),
                service: service.to_owned(),
                error_class: "RuntimeError".to_owned(),
                message: message.to_owned(),
                message_template: message.to_owned(),
                stack_trace: None,
                context_json: None,
                severity: "error".to_owned(),
                environment: "production".to_owned(),
                group_hash: group_hash.to_owned(),
                fingerprint_json: None,
                region: None,
                classification: Classification {
                    category: Category::Application,
                    persistence: Persistence::Persistent,
                    component: Component::Runtime,
                },
                created_at: server_time::current_rfc3339(),
            },
        })
    }

    fn valid_error_body() -> &'static str {
        r#"{"service":"test-svc","error_class":"RuntimeError","message":"something went wrong"}"#
    }

    fn valid_check_in_body() -> &'static str {
        r#"{"monitor":"desktop-active-timer","status":"alive","observed_at":"2026-05-28T20:00:00Z","ttl_ms":120000}"#
    }

    fn runtime_store(active_webhook: bool) -> Result<SharedStore, Box<dyn Error>> {
        runtime_store_with_url(active_webhook, "https://example.test/hook")
    }

    fn runtime_store_with_url(
        active_webhook: bool,
        url: &str,
    ) -> Result<SharedStore, Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        store.insert_webhook_subscription(webhook_subscription_insert(
            "WHK-test",
            url,
            vec!["error.new_class".to_owned()],
            "test-webhook-secret",
            active_webhook,
            "2026-05-28T20:00:00Z",
        ))?;

        Ok(Arc::new(parking_lot::Mutex::new(store)))
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
            attempt_timestamp: None,
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

    fn local_webhook_transport_builder() -> Result<Arc<dyn WebhookTransport>, String> {
        let transport = thread::Builder::new()
            .name("canary-test-webhook-transport-init".to_owned())
            .spawn(|| {
                HttpWebhookTransport::with_timeout_allowing_private_destinations(
                    StdDuration::from_secs(10),
                )
            })
            .map_err(|error| {
                format!("failed to spawn test webhook transport initializer: {error}")
            })?
            .join()
            .map_err(|_| "test webhook transport initializer panicked".to_owned())??;
        Ok(Arc::new(transport))
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
        store: &SharedStore,
        delivery_id: &str,
        max_attempts: u32,
    ) -> Result<i64, Box<dyn Error>> {
        let mut store = store.lock();
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

    fn assert_webhook_drain_report(
        report: &WebhookDeliveryDrainReport,
        claimed: u32,
        completed: u32,
        retried: u32,
        discarded: u32,
    ) {
        assert_eq!(report.claimed, claimed);
        assert_eq!(report.completed, completed);
        assert_eq!(report.retried, retried);
        assert_eq!(report.discarded, discarded);
        assert_eq!(report.recovered, 0);
        assert_eq!(report.recovery_retried, 0);
        assert_eq!(report.recovery_discarded, 0);
        assert_eq!(report.circuit_open_suppressed, 0);
    }

    fn wait_for_delivery_status(
        store: &SharedStore,
        delivery_id: &str,
        status: WebhookDeliveryStatus,
    ) -> Result<(), Box<dyn Error>> {
        let deadline = Instant::now() + StdDuration::from_secs(2);
        loop {
            {
                let store = store.lock();
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

        fn request_error(reason: impl Into<String>) -> Self {
            Self {
                response: TransportResult::RequestError(reason.into()),
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

    impl RecordingScheduler {
        fn jobs(&self) -> Result<Vec<WebhookJob>, String> {
            self.jobs
                .lock()
                .map_err(|_| "scheduler lock poisoned".to_owned())
                .map(|jobs| jobs.clone())
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
