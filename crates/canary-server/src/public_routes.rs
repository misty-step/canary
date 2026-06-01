//! Axum adapter for Canary's public unauthenticated endpoints.
//!
//! The public wire bodies live in `canary-http::public`; this module only binds
//! those stable contracts to paths and response conversion in the server crate.

use axum::{Router, body::Body, extract::State, http::Response, routing::get};
use canary_http::public::{DependencyStatus, healthz_response, openapi_response, readyz_response};

use crate::http_contract::{json_response, text_response};

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

async fn healthz() -> Response<Body> {
    json_response(healthz_response())
}

async fn readyz(State(readiness): State<PublicReadiness>) -> Response<Body> {
    json_response(readyz_response(readiness.database, readiness.supervisor))
}

async fn openapi() -> Response<Body> {
    text_response(openapi_response())
}
