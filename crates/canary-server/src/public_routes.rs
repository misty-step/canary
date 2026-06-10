//! Axum adapter for Canary's public unauthenticated endpoints.
//!
//! The public wire bodies live in `canary-http::public`; this module only binds
//! those stable contracts to paths and response conversion in the server crate.

use std::sync::Arc;

use axum::{Router, body::Body, extract::State, http::Response, routing::get};
use canary_http::public::{DependencyStatus, healthz_response, openapi_response, readyz_response};

use crate::http_contract::{json_response, text_response};

/// Snapshot of dependency readiness for the public readiness endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicReadinessSnapshot {
    /// Writable SQLite dependency status.
    pub database: DependencyStatus,
    /// Runtime supervisor dependency status.
    pub supervisor: DependencyStatus,
}

/// Live readiness source for the public readiness endpoint.
pub trait PublicReadinessProbe: Send + Sync + 'static {
    /// Return the current dependency snapshot for this process.
    fn snapshot(&self) -> PublicReadinessSnapshot;
}

/// Public readiness endpoint state.
#[derive(Clone)]
pub struct PublicReadiness {
    probe: Arc<dyn PublicReadinessProbe>,
}

impl PublicReadiness {
    /// Build readiness from explicit dependency statuses.
    pub fn new(database: DependencyStatus, supervisor: DependencyStatus) -> Self {
        Self::from_probe(Arc::new(StaticPublicReadiness {
            snapshot: PublicReadinessSnapshot {
                database,
                supervisor,
            },
        }))
    }

    /// Build readiness from a live dependency probe.
    pub fn from_probe(probe: Arc<dyn PublicReadinessProbe>) -> Self {
        Self { probe }
    }

    /// Convenience constructor for a fully ready process.
    pub fn ready() -> Self {
        Self::new(DependencyStatus::Ok, DependencyStatus::Ok)
    }

    fn snapshot(&self) -> PublicReadinessSnapshot {
        self.probe.snapshot()
    }
}

struct StaticPublicReadiness {
    snapshot: PublicReadinessSnapshot,
}

impl PublicReadinessProbe for StaticPublicReadiness {
    fn snapshot(&self) -> PublicReadinessSnapshot {
        self.snapshot
    }
}

impl PublicReadinessSnapshot {
    /// Build a readiness snapshot from explicit dependency statuses.
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
    let snapshot = readiness.snapshot();
    json_response(readyz_response(snapshot.database, snapshot.supervisor))
}

async fn openapi() -> Response<Body> {
    text_response(openapi_response())
}
