//! Axum adapter for Canary's admin webhook routes.
//!
//! Webhook delivery execution and delivery read models stay outside this module.
//! This adapter owns only webhook subscription mutation and the explicit
//! admin-triggered test delivery route.

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::{
    auth::Permission,
    problem_details::{
        ProblemDetails, internal_problem, not_found_problem, payload_too_large_problem,
        validation_detail_problem, webhook_delivery_failed_problem,
    },
    request::{MAX_JSON_BODY_BYTES, decode_json_object},
};
use canary_ingest::ValidationErrors;
use canary_store::{WebhookSubscription, WebhookSubscriptionInsert};
use canary_workers::webhooks::{TransportResult, WebhookEndpoint, WebhookJob, build_request};
use serde_json::{Map, Value, json};

use crate::{
    IngestState, check_content_length, current_rfc3339, json_status_response, problem_response,
    require_scope, required_string, required_string_array, response,
};

pub(crate) async fn list_webhooks(
    State(state): State<IngestState>,
    headers: HeaderMap,
) -> Response<Body> {
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

pub(crate) async fn create_webhook(
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

pub(crate) async fn delete_webhook(
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

pub(crate) async fn test_webhook(
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

fn parse_webhook_create(
    attrs: Map<String, Value>,
) -> Result<WebhookSubscriptionInsert, Box<ProblemDetails>> {
    let mut errors: ValidationErrors = ValidationErrors::new();
    let url = required_string(&attrs, "url", &mut errors);
    let events = required_string_array(&attrs, "events", &mut errors);

    if !errors.is_empty() {
        return Err(Box::new(validation_detail_problem(
            "Invalid webhook configuration.",
        )));
    }

    let Some(url) = url else {
        return Err(Box::new(validation_detail_problem(
            "Invalid webhook configuration.",
        )));
    };
    let Some(events) = events else {
        return Err(Box::new(validation_detail_problem(
            "Invalid webhook configuration.",
        )));
    };

    let invalid = events
        .iter()
        .filter(|event| !canary_core::webhook_events::valid(event))
        .cloned()
        .collect::<Vec<_>>();
    if !invalid.is_empty() {
        return Err(Box::new(validation_detail_problem(format!(
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
