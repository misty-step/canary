//! Axum server wiring for Canary.
//!
//! This crate adapts the stable wire contracts from `canary-http` to concrete
//! HTTP responses. Domain decisions and body shapes stay out of the router.

use axum::{
    Router,
    http::header::{CONTENT_TYPE, HeaderName},
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
mod body_fields;
mod health_fanout;
mod health_routes;
mod http_contract;
mod ingest_routes;
mod metrics_routes;
mod monitor_overdue;
mod public_routes;
mod query_routes;
mod rate_limit;
mod report_routes;
mod retention_prune;
mod route_state;
mod runtime;
mod runtime_env;
mod server_auth;
mod server_time;
mod service_onboarding_routes;
mod target_probes;
mod target_request;
mod tls_scan;
mod webhook_delivery;
mod webhook_delivery_routes;
mod webhooks;

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
pub use health_fanout::{
    EnqueueFailure, EnqueueFailureKey, EnqueueFailureRecorder, EnqueueFailureSink,
    EventFanoutReport, HealthEventFanout, HealthEventSource,
};
use health_routes::{health_status, status, target_checks};
use ingest_routes::{create_check_in, create_error};
use metrics_routes::metrics;
pub use monitor_overdue::{
    MonitorOverdueLifecycle, MonitorOverdueLifecycleConfig, MonitorOverdueLifecycleReport,
    MonitorOverdueLifecycleWorker, MonitorOverdueOutcome, MonitorOverdueRuntime,
    MonitorOverdueRuntimeError, run_monitor_overdue_once,
};
pub use public_routes::{PublicReadiness, public_router};
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
    require_ingest_scope, require_query_limited_admin_scope, require_read_scope, require_scope,
};
use service_onboarding_routes::create_service_onboarding;
pub use target_probes::{
    ProbeHttpResponse, ProbeRequest, ProbeTransport, ProbeTransportError, ReqwestProbeTransport,
    TargetProbeLifecycle, TargetProbeLifecycleCommand, TargetProbeLifecycleConfig,
    TargetProbeLifecycleController, TargetProbeLifecycleReport, TargetProbeLifecycleWorker,
    TargetProbeOptions, TargetProbeOutcome, TargetProbeRuntime, TargetProbeRuntimeError,
    run_target_probe_once, validate_target_configuration,
};
pub use tls_scan::{
    TlsExpiryScanLifecycle, TlsExpiryScanLifecycleConfig, TlsExpiryScanLifecycleReport,
    TlsExpiryScanLifecycleWorker, TlsExpiryScanRuntimeError, run_tls_expiry_scan_once,
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
        .route(
            "/api/v1/webhook-deliveries/{delivery_id}",
            get(webhook_delivery),
        )
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
    use std::sync::{
        Arc, Mutex, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };
    use std::thread::{self, JoinHandle, ThreadId};
    use std::time::{Duration as StdDuration, Instant};

    use axum::{
        body::{Body, to_bytes},
        http::{
            HeaderMap, HeaderValue, Method, Request, Response, StatusCode, header::CONTENT_TYPE,
        },
    };
    use canary_core::{
        ids::{ErrorId, EventId},
        ingest::classification::{Category, Classification, Component, Persistence},
    };
    use canary_http::{
        public::{APPLICATION_JSON, DependencyStatus, OPENAPI_JSON},
        request::MAX_JSON_BODY_BYTES,
    };
    use canary_ingest::{IngestConfig, IngestEffect};
    use canary_store::{
        API_KEY_PREFIX_LEN, ApiKeyInsert, ErrorIngest, ErrorIngestIds, ErrorIngestPayload,
        MonitorInsert, Store, TargetCheckObservation, TargetInsert, TargetProbeCommit,
        WebhookDeliveryInsert, WebhookDeliveryJobInsert, WebhookDeliveryJobState,
        WebhookDeliveryStatus, WebhookSubscriptionInsert,
    };
    use canary_workers::webhooks::{CircuitDecision, TransportResult, WebhookJob, WebhookRequest};
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
            "boot should seed only the one-time bootstrap admin key"
        );
        assert!(server.enqueue_failure_snapshot().is_empty());
        assert_eq!(server.retention_prune_failure_count(), 0);
        assert_eq!(server.tls_expiry_scan_failure_count(), 0);

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

    #[tokio::test]
    async fn ingest_router_mounts_authenticated_route_matrix() -> Result<(), Box<dyn Error>> {
        let router = ingest_router(test_ingest_state()?);
        let routes = [
            (Method::GET, "/metrics"),
            (Method::POST, "/api/v1/errors"),
            (Method::POST, "/api/v1/check-ins"),
            (Method::GET, "/api/v1/query"),
            (Method::GET, "/api/v1/report"),
            (Method::GET, "/api/v1/timeline"),
            (Method::GET, "/api/v1/webhook-deliveries"),
            (Method::GET, "/api/v1/webhook-deliveries/DLV-route"),
            (Method::GET, "/api/v1/status"),
            (Method::GET, "/api/v1/health-status"),
            (Method::GET, "/api/v1/targets/TGT-route/checks"),
            (Method::GET, "/api/v1/incidents"),
            (Method::GET, "/api/v1/incidents/INC-route"),
            (Method::GET, "/api/v1/incidents/INC-route/annotations"),
            (Method::POST, "/api/v1/incidents/INC-route/annotations"),
            (Method::GET, "/api/v1/groups/group-route/annotations"),
            (Method::POST, "/api/v1/groups/group-route/annotations"),
            (Method::GET, "/api/v1/annotations"),
            (Method::POST, "/api/v1/annotations"),
            (Method::GET, "/api/v1/errors/ERR-route"),
            (Method::GET, "/api/v1/monitors"),
            (Method::POST, "/api/v1/monitors"),
            (Method::DELETE, "/api/v1/monitors/MON-route"),
            (Method::GET, "/api/v1/webhooks"),
            (Method::POST, "/api/v1/webhooks"),
            (Method::DELETE, "/api/v1/webhooks/WHK-route"),
            (Method::POST, "/api/v1/webhooks/WHK-route/test"),
            (Method::GET, "/api/v1/keys"),
            (Method::POST, "/api/v1/keys"),
            (Method::POST, "/api/v1/keys/KEY-route/revoke"),
            (Method::POST, "/api/v1/service-onboarding"),
            (Method::GET, "/api/v1/targets"),
            (Method::POST, "/api/v1/targets"),
            (Method::PATCH, "/api/v1/targets/TGT-route"),
            (Method::DELETE, "/api/v1/targets/TGT-route"),
            (Method::POST, "/api/v1/targets/TGT-route/pause"),
            (Method::POST, "/api/v1/targets/TGT-route/resume"),
        ];

        for (method, path) in routes {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method.clone())
                        .uri(path)
                        .body(Body::empty())?,
                )
                .await?;

            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
            let body = json_body(response).await?;
            assert_eq!(body["code"], "invalid_api_key", "{method} {path}");
        }

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
    async fn canary_server_boot_wires_retention_prune_worker() -> Result<(), Box<dyn Error>> {
        let path = temp_db_path("retention-prune");
        {
            let mut store = Store::open(&path)?;
            store.migrate()?;
            for index in 0..1005 {
                store.commit_error_ingest(test_error_ingest(index, "2026-04-01T00:00:00Z"))?;
            }
            store.commit_error_ingest(test_error_ingest(2000, "2026-05-28T00:00:00Z"))?;
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
        let store = store.lock().map_err(|_| "store lock poisoned")?;
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
            store.insert_webhook_subscription(WebhookSubscriptionInsert {
                id: "WHK-inactive-test".to_owned(),
                url: "https://example.com/inactive".to_owned(),
                events: vec!["canary.ping".to_owned()],
                secret: "inactive-secret".to_owned(),
                active: false,
                created_at: "2026-06-01T00:00:00Z".to_owned(),
            })?;
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
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
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
            store.create_pending_webhook_delivery(WebhookDeliveryInsert {
                delivery_id: "DLV-diagnostic-show".to_owned(),
                webhook_id: "WHK-diagnostic".to_owned(),
                event: "incident.updated".to_owned(),
                now: "2026-04-02T10:00:00Z".to_owned(),
            })?;
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
            "API key scope `ingest-only` cannot access this read endpoint. Use an `admin` or `read-only` key."
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
                "API key scope `ingest-only` cannot access this read endpoint. Use an `admin` or `read-only` key.",
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
            let mut store = state.lock_store().map_err(|_| "store lock poisoned")?;
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
                signal_type: "error_group".to_owned(),
                signal_ref: first_group,
                service: "api".to_owned(),
                incident_id: annotated_id.parse()?,
                event_id: canary_core::ids::EventId::generate(),
                now: "2026-05-28T20:00:00Z".to_owned(),
            })?;
            let plain_id = canary_core::ids::IncidentId::generate().into_string();
            store.correlate_incident(IncidentCorrelation {
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
    async fn ingest_and_query_routes_enforce_phoenix_rate_limit_buckets()
    -> Result<(), Box<dyn Error>> {
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

    fn test_error_ingest(index: usize, created_at: &str) -> ErrorIngest {
        ErrorIngest {
            ids: ErrorIngestIds {
                error_id: ErrorId::generate(),
                event_id: EventId::generate(),
            },
            payload: ErrorIngestPayload {
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

    fn valid_error_body() -> &'static str {
        r#"{"service":"test-svc","error_class":"RuntimeError","message":"something went wrong"}"#
    }

    fn valid_check_in_body() -> &'static str {
        r#"{"monitor":"desktop-active-timer","status":"alive","observed_at":"2026-05-28T20:00:00Z","ttl_ms":120000}"#
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
