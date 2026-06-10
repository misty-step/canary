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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyzChecks {
    /// Database check result.
    pub database: DependencyStatus,
    /// Supervisor check result.
    pub supervisor: DependencyStatus,
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
pub const fn readyz_response(
    database: DependencyStatus,
    supervisor: DependencyStatus,
) -> PublicResponse<ReadyzResponse> {
    let all_ok =
        matches!(database, DependencyStatus::Ok) && matches!(supervisor, DependencyStatus::Ok);
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
        let response = readyz_response(DependencyStatus::Ok, DependencyStatus::Ok);

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, APPLICATION_JSON);
        assert_eq!(
            serde_json::to_value(response.body).unwrap_or(Value::Null),
            json!({
                "status": "ready",
                "checks": {
                    "database": "ok",
                    "supervisor": "ok"
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
            let response = readyz_response(database, supervisor);
            let body = serde_json::to_value(response.body).unwrap_or(Value::Null);

            assert_eq!(response.status, 503);
            assert_eq!(body["status"], "not_ready");
        }
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
