//! Public, unauthenticated HTTP endpoint contracts.
//!
//! These helpers model Phoenix's public routes without committing the rewrite
//! to a router framework. The future Axum handlers should be thin adapters over
//! these response builders.

use serde::{Deserialize, Serialize};

/// The OpenAPI document served by `GET /api/v1/openapi.json`.
pub const OPENAPI_JSON: &str = include_str!("../../../priv/openapi/openapi.json");

/// JSON content type observed from Phoenix for these public endpoints.
pub const APPLICATION_JSON: &str = "application/json; charset=utf-8";

/// Public route contract for unauthenticated endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicRoute {
    /// HTTP method.
    pub method: &'static str,
    /// Route path.
    pub path: &'static str,
    /// Whether this route intentionally bypasses API-key authentication.
    pub unauthenticated: bool,
    /// Whether this route intentionally bypasses ingest/query rate limiting.
    pub rate_limited: bool,
}

/// Public routes that must remain outside authenticated and rate-limited router
/// pipelines.
pub const PUBLIC_ROUTES: &[PublicRoute] = &[
    PublicRoute {
        method: "GET",
        path: "/healthz",
        unauthenticated: true,
        rate_limited: false,
    },
    PublicRoute {
        method: "GET",
        path: "/readyz",
        unauthenticated: true,
        rate_limited: false,
    },
    PublicRoute {
        method: "GET",
        path: "/api/v1/openapi.json",
        unauthenticated: true,
        rate_limited: false,
    },
];

/// Minimal HTTP response contract for framework adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicResponse<T> {
    /// HTTP status code.
    pub status: u16,
    /// Phoenix-compatible response content type.
    pub content_type: &'static str,
    /// Response body.
    pub body: T,
}

/// Liveness response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthzResponse {
    /// Stable Phoenix liveness marker.
    pub status: HealthzStatus,
}

/// Liveness status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthzStatus {
    /// Process is alive.
    Ok,
}

/// Readiness response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyzResponse {
    /// Overall readiness status.
    pub status: ReadyzStatus,
    /// Individual dependency checks.
    pub checks: ReadyzChecks,
}

/// Readiness status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadyzStatus {
    /// All dependencies are ready.
    Ready,
    /// At least one dependency failed.
    NotReady,
}

/// Public readiness dependency checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyzChecks {
    /// Database check result.
    pub database: DependencyStatus,
    /// Supervisor check result.
    pub supervisor: DependencyStatus,
    /// Background worker lifecycle checks.
    pub workers: Vec<WorkerReadyzCheck>,
}

/// Dependency check status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyStatus {
    /// Dependency is healthy.
    Ok,
    /// Dependency check failed.
    Error,
}

/// Background worker lifecycle readiness check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerReadyzCheck {
    /// Stable worker name.
    pub name: String,
    /// Whether the worker thread is currently running.
    pub state: WorkerLifecycleState,
    /// Derived health classification for readiness policy.
    pub health: WorkerHealthStatus,
    /// Last successful lifecycle pass timestamp.
    pub last_success_at: Option<String>,
    /// Milliseconds since the last successful lifecycle pass, when known.
    pub last_success_age_ms: Option<u64>,
    /// Count of runtime errors or panics observed by the worker loop.
    pub failure_count: u64,
    /// Consecutive runtime failures since the last successful lifecycle pass.
    pub consecutive_failures: u64,
    /// Last non-secret failure class observed by the worker loop.
    pub last_error_class: Option<String>,
    /// Work items due at the last observed lifecycle pass.
    pub due_count: u64,
    /// Work items still in flight at the last observed lifecycle pass.
    pub in_flight_count: u64,
    /// Milliseconds by which the oldest due work item is overdue, when known.
    pub oldest_due_age_ms: Option<u64>,
    /// Whether the last observed pass saw backoff, circuit-open, or interruption pressure.
    pub backoff_or_circuit_open: bool,
}

/// Background worker lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerLifecycleState {
    /// Worker thread is running.
    Started,
    /// Worker thread has stopped.
    Stopped,
}

/// Background worker readiness health classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerHealthStatus {
    /// Worker is started, fresh, and below pressure thresholds.
    Ok,
    /// Worker has not completed a successful pass recently enough.
    Stale,
    /// Worker has crossed the repeated-failure threshold.
    Failing,
    /// Worker is alive but work pressure is above policy.
    Pressured,
    /// Worker thread is stopped.
    Stopped,
}

/// Build the Phoenix-compatible `GET /healthz` response.
pub const fn healthz_response() -> PublicResponse<HealthzResponse> {
    PublicResponse {
        status: 200,
        content_type: APPLICATION_JSON,
        body: HealthzResponse {
            status: HealthzStatus::Ok,
        },
    }
}

/// Build the Phoenix-compatible `GET /readyz` response.
pub fn readyz_response(
    database: DependencyStatus,
    supervisor: DependencyStatus,
    workers: Vec<WorkerReadyzCheck>,
) -> PublicResponse<ReadyzResponse> {
    let all_ok = matches!(database, DependencyStatus::Ok)
        && matches!(supervisor, DependencyStatus::Ok)
        && workers.iter().all(|worker| {
            matches!(worker.state, WorkerLifecycleState::Started)
                && matches!(
                    worker.health,
                    WorkerHealthStatus::Ok | WorkerHealthStatus::Pressured
                )
        });
    let status = if all_ok { 200 } else { 503 };
    let body_status = if all_ok {
        ReadyzStatus::Ready
    } else {
        ReadyzStatus::NotReady
    };

    PublicResponse {
        status,
        content_type: APPLICATION_JSON,
        body: ReadyzResponse {
            status: body_status,
            checks: ReadyzChecks {
                database,
                supervisor,
                workers,
            },
        },
    }
}

/// Build the Phoenix-compatible `GET /api/v1/openapi.json` response.
pub const fn openapi_response() -> PublicResponse<&'static str> {
    PublicResponse {
        status: 200,
        content_type: APPLICATION_JSON,
        body: OPENAPI_JSON,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn healthz_matches_phoenix_body_and_status() {
        let response = healthz_response();

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, APPLICATION_JSON);
        assert_eq!(
            serde_json::to_value(response.body).unwrap_or(Value::Null),
            json!({"status": "ok"})
        );
    }

    #[test]
    fn readyz_matches_phoenix_body_for_healthy_dependencies() {
        let response = readyz_response(DependencyStatus::Ok, DependencyStatus::Ok, Vec::new());

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, APPLICATION_JSON);
        assert_eq!(
            serde_json::to_value(response.body).unwrap_or(Value::Null),
            json!({
                "status": "ready",
                "checks": {
                    "database": "ok",
                    "supervisor": "ok",
                    "workers": []
                }
            })
        );
    }

    #[test]
    fn readyz_matches_phoenix_body_for_failed_dependencies() {
        let cases = [
            (DependencyStatus::Error, DependencyStatus::Ok),
            (DependencyStatus::Ok, DependencyStatus::Error),
            (DependencyStatus::Error, DependencyStatus::Error),
        ];

        for (database, supervisor) in cases {
            let response = readyz_response(database, supervisor, Vec::new());
            let body = serde_json::to_value(response.body).unwrap_or(Value::Null);

            assert_eq!(response.status, 503);
            assert_eq!(body["status"], "not_ready");
        }
    }

    #[test]
    fn readyz_marks_stopped_workers_not_ready_without_error_details() {
        let response = readyz_response(
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
                    due_count: 0,
                    in_flight_count: 0,
                    oldest_due_age_ms: None,
                    backoff_or_circuit_open: false,
                },
                WorkerReadyzCheck {
                    name: "target_probe".to_owned(),
                    state: WorkerLifecycleState::Stopped,
                    health: WorkerHealthStatus::Stopped,
                    last_success_at: None,
                    last_success_age_ms: None,
                    failure_count: 2,
                    consecutive_failures: 2,
                    last_error_class: Some("panic".to_owned()),
                    due_count: 0,
                    in_flight_count: 0,
                    oldest_due_age_ms: None,
                    backoff_or_circuit_open: false,
                },
            ],
        );
        let body = serde_json::to_value(response.body).unwrap_or(Value::Null);

        assert_eq!(response.status, 503);
        assert_eq!(body["status"], "not_ready");
        assert_eq!(body["checks"]["workers"][0]["name"], "webhook_delivery");
        assert_eq!(body["checks"]["workers"][0]["state"], "started");
        assert_eq!(body["checks"]["workers"][1]["state"], "stopped");
        assert_eq!(body["checks"]["workers"][1]["health"], "stopped");
        assert_eq!(body["checks"]["workers"][1]["last_error_class"], "panic");
        assert_eq!(body["checks"]["workers"][1].get("last_error"), None);
    }

    #[test]
    fn readyz_marks_unhealthy_started_workers_not_ready() {
        for health in [WorkerHealthStatus::Stale, WorkerHealthStatus::Failing] {
            let response = readyz_response(
                DependencyStatus::Ok,
                DependencyStatus::Ok,
                vec![WorkerReadyzCheck {
                    name: "webhook_delivery".to_owned(),
                    state: WorkerLifecycleState::Started,
                    health,
                    last_success_at: Some("2026-06-12T20:00:00Z".to_owned()),
                    last_success_age_ms: Some(120_000),
                    failure_count: 3,
                    consecutive_failures: 3,
                    last_error_class: Some("runtime_error".to_owned()),
                    due_count: 12,
                    in_flight_count: 0,
                    oldest_due_age_ms: Some(90_000),
                    backoff_or_circuit_open: true,
                }],
            );
            let body = serde_json::to_value(response.body).unwrap_or(Value::Null);

            assert_eq!(response.status, 503);
            assert_eq!(body["status"], "not_ready");
        }
    }

    #[test]
    fn readyz_keeps_pressured_started_workers_route_ready() {
        let response = readyz_response(
            DependencyStatus::Ok,
            DependencyStatus::Ok,
            vec![WorkerReadyzCheck {
                name: "monitor_overdue".to_owned(),
                state: WorkerLifecycleState::Started,
                health: WorkerHealthStatus::Pressured,
                last_success_at: Some("2026-06-12T20:00:00Z".to_owned()),
                last_success_age_ms: Some(735),
                failure_count: 0,
                consecutive_failures: 0,
                last_error_class: None,
                due_count: 1,
                in_flight_count: 0,
                oldest_due_age_ms: Some(690_853),
                backoff_or_circuit_open: false,
            }],
        );
        let body = serde_json::to_value(response.body).unwrap_or(Value::Null);

        assert_eq!(response.status, 200);
        assert_eq!(body["status"], "ready");
        assert_eq!(body["checks"]["workers"][0]["name"], "monitor_overdue");
        assert_eq!(body["checks"]["workers"][0]["state"], "started");
        assert_eq!(body["checks"]["workers"][0]["health"], "pressured");
        assert_eq!(body["checks"]["workers"][0]["due_count"], 1);
        assert_eq!(body["checks"]["workers"][0]["oldest_due_age_ms"], 690_853);
    }

    #[test]
    fn openapi_response_serves_the_checked_in_contract_unchanged() {
        let response = openapi_response();
        let document: Value = serde_json::from_str(response.body).unwrap_or(Value::Null);

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, APPLICATION_JSON);
        assert_eq!(document["openapi"], "3.1.0");
        assert_eq!(document["info"]["title"], "Canary API");
        assert_eq!(document["paths"]["/healthz"]["get"]["security"], json!([]));
        assert_eq!(document["paths"]["/readyz"]["get"]["security"], json!([]));
        assert_eq!(
            document["paths"]["/api/v1/openapi.json"]["get"]["security"],
            json!([])
        );
        assert_eq!(
            document["components"]["schemas"]["ReadyzResponse"]["required"],
            json!(["status", "checks"])
        );
        assert_eq!(
            document["components"]["schemas"]["ReadyzResponse"]["properties"]["checks"]["required"],
            json!(["database", "supervisor", "workers"])
        );
        assert_eq!(
            document["components"]["schemas"]["WorkerReadyzCheck"]["required"],
            json!([
                "name",
                "state",
                "health",
                "last_success_at",
                "last_success_age_ms",
                "failure_count",
                "consecutive_failures",
                "last_error_class",
                "due_count",
                "in_flight_count",
                "oldest_due_age_ms",
                "backoff_or_circuit_open"
            ])
        );
    }

    #[test]
    fn public_routes_are_explicitly_unauthenticated_and_not_rate_limited() {
        assert_eq!(
            PUBLIC_ROUTES,
            &[
                PublicRoute {
                    method: "GET",
                    path: "/healthz",
                    unauthenticated: true,
                    rate_limited: false,
                },
                PublicRoute {
                    method: "GET",
                    path: "/readyz",
                    unauthenticated: true,
                    rate_limited: false,
                },
                PublicRoute {
                    method: "GET",
                    path: "/api/v1/openapi.json",
                    unauthenticated: true,
                    rate_limited: false,
                },
            ]
        );
    }
}
