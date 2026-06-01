//! Axum adapter for Canary's Prometheus scrape route.
//!
//! Metrics are an admin-only read surface. The route owns auth, store snapshot
//! loading, and text exposition response conversion; metric rendering stays in
//! `canary-core`.

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::problem_details::internal_problem;

use crate::{IngestState, problem_response, require_query_limited_admin_scope, response};

pub(crate) const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

pub(crate) async fn metrics(
    State(state): State<IngestState>,
    headers: HeaderMap,
) -> Response<Body> {
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
