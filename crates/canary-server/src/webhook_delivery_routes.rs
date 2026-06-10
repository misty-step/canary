//! Axum adapter for Canary's webhook delivery ledger read route.
//!
//! Delivery execution and retry policy live in `webhooks`; durable page queries
//! live in `canary-store`. This module only translates the authenticated HTTP
//! read contract for agents inspecting delivery outcomes.

use axum::{
    body::Body,
    extract::{Path, Query, RawQuery, State},
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::problem_details::{
    internal_problem, invalid_cursor_problem, invalid_limit_problem, invalid_string_param_problem,
    invalid_webhook_delivery_status_problem, not_found_problem,
};
use canary_store::{WebhookDeliveryPageError, WebhookDeliveryPageOptions};
use serde::Deserialize;

use crate::{
    IngestState,
    http_contract::{json_status_response, problem_response, query_param_is_array},
    require_read_scope,
};

#[derive(Deserialize)]
pub(crate) struct WebhookDeliveryParams {
    webhook_id: Option<String>,
    event: Option<String>,
    status: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
    after: Option<String>,
}

pub(crate) async fn webhook_deliveries(
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
    let store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.webhook_delivery_page(options) {
        Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
        Err(WebhookDeliveryPageError::InvalidLimit) => problem_response(invalid_limit_problem()),
        Err(WebhookDeliveryPageError::InvalidCursor) => problem_response(invalid_cursor_problem()),
        Err(WebhookDeliveryPageError::InvalidStatus) => problem_response(
            invalid_webhook_delivery_status_problem(canary_store::webhook_delivery_statuses()),
        ),
        Err(WebhookDeliveryPageError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

pub(crate) async fn webhook_delivery(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(delivery_id): Path<String>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.webhook_delivery(&delivery_id) {
        Ok(Some(delivery)) => json_status_response(StatusCode::OK.as_u16(), delivery),
        Ok(None) => problem_response(not_found_problem(format!(
            "Webhook delivery {delivery_id} not found."
        ))),
        Err(_) => problem_response(internal_problem()),
    }
}
