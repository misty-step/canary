use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration as StdDuration, Instant},
};

use canary_ingest::IngestEffect;
use canary_store::{
    BOOTSTRAP_PROJECT_ID, BOOTSTRAP_TENANT_ID, WebhookDeliveryInsert, WebhookDeliveryJobInsert,
    WebhookSubscription,
};
use canary_workers::webhooks::{
    TransportResult, WebhookEndpoint, WebhookEnqueueDecision, WebhookJob, WebhookRequest,
    plan_enqueue_for_event,
};
use serde_json::{Value, json};

use crate::{
    IngestEffectSink,
    egress::{ValidatedHttpDestination, validate_public_http_destination},
    route_state::SharedStore,
    server_time::current_rfc3339,
};

/// Runtime boundary for scheduling webhook delivery jobs.
pub trait WebhookScheduler: Send + Sync + 'static {
    /// Schedule one webhook job after its pending ledger row has been created.
    fn schedule(&self, job: &WebhookJob) -> Result<(), String>;
}

/// Store-backed scheduler for webhook delivery jobs.
pub struct StoreWebhookScheduler {
    store: SharedStore,
}

impl StoreWebhookScheduler {
    /// Build a scheduler backed by the shared single-writer store.
    pub fn new(store: SharedStore) -> Self {
        Self { store }
    }
}

impl WebhookScheduler for StoreWebhookScheduler {
    fn schedule(&self, job: &WebhookJob) -> Result<(), String> {
        let now = current_rfc3339();
        let mut store = self.store.lock();
        store
            .insert_webhook_delivery_job(WebhookDeliveryJobInsert {
                args: job_args(job),
                scheduled_at: now.clone(),
                now,
                max_attempts: job.effective_max_attempts(),
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Runtime boundary for webhook cooldown state.
pub trait WebhookCooldown: Send + Sync + 'static {
    /// Return true when the event should be suppressed.
    fn in_cooldown(&self, key: &str) -> bool;

    /// Mark a key after the scheduler accepts a job.
    fn mark(&self, key: &str);
}

/// In-process cooldown state for webhook enqueue suppression.
///
/// The previous Elixir implementation stored this in ETS. The Rust server keeps the same process-local
/// contract explicitly here: cooldown state is advisory, monotonic, and lost on
/// restart.
#[derive(Debug)]
pub struct InMemoryWebhookCooldown {
    ttl: StdDuration,
    marked_at: Mutex<HashMap<String, Instant>>,
}

impl InMemoryWebhookCooldown {
    const DEFAULT_TTL: StdDuration = StdDuration::from_secs(5 * 60);

    /// Build the five-minute webhook cooldown.
    pub fn default_config() -> Self {
        Self::with_ttl(Self::DEFAULT_TTL)
    }

    /// Build cooldown state with an explicit TTL for tests.
    pub fn with_ttl(ttl: StdDuration) -> Self {
        Self {
            ttl,
            marked_at: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryWebhookCooldown {
    fn default() -> Self {
        Self::default_config()
    }
}

impl WebhookCooldown for InMemoryWebhookCooldown {
    fn in_cooldown(&self, key: &str) -> bool {
        let Ok(marked_at) = self.marked_at.lock() else {
            return false;
        };
        marked_at
            .get(key)
            .is_some_and(|marked_at| marked_at.elapsed() < self.ttl)
    }

    fn mark(&self, key: &str) {
        let Ok(mut marked_at) = self.marked_at.lock() else {
            return;
        };
        marked_at.insert(key.to_owned(), Instant::now());
    }
}

/// Runtime boundary for outbound webhook transport.
pub trait WebhookTransport: Send + Sync + 'static {
    /// Send one signed webhook request.
    fn send(&self, request: &WebhookRequest) -> TransportResult;
}

/// Concrete HTTP transport for outbound webhook delivery.
pub struct HttpWebhookTransport {
    timeout: StdDuration,
    allow_private_destinations: bool,
}

impl HttpWebhookTransport {
    const DEFAULT_TIMEOUT: StdDuration = StdDuration::from_secs(10);
    const DEFAULT_CONNECT_TIMEOUT: StdDuration = StdDuration::from_secs(3);
    const USER_AGENT: &'static str = concat!("canary-server/", env!("CARGO_PKG_VERSION"));

    /// Build an HTTP transport with timeout and no redirects.
    ///
    /// TLS certificate validation stays enabled. The blocking send path is meant
    /// for the webhook drain worker, not an Axum request task.
    pub fn try_new() -> Result<Self, String> {
        Self::with_timeout(Self::DEFAULT_TIMEOUT)
    }

    /// Build an HTTP transport with an explicit timeout and no hidden retries.
    pub fn with_timeout(timeout: StdDuration) -> Result<Self, String> {
        Self::with_timeout_and_private_destinations(timeout, false)
    }

    #[cfg(test)]
    pub(crate) fn with_timeout_allowing_private_destinations(
        timeout: StdDuration,
    ) -> Result<Self, String> {
        Self::with_timeout_and_private_destinations(timeout, true)
    }

    fn with_timeout_and_private_destinations(
        timeout: StdDuration,
        allow_private_destinations: bool,
    ) -> Result<Self, String> {
        let _ = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(Self::DEFAULT_CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .http1_only()
            .user_agent(Self::USER_AGENT)
            .no_proxy()
            .build()
            .map_err(|error| format!("failed to build webhook HTTP transport: {error}"))?;

        Ok(Self {
            timeout,
            allow_private_destinations,
        })
    }

    fn client_for_destination(
        &self,
        destination: Option<&ValidatedHttpDestination>,
    ) -> Result<reqwest::blocking::Client, String> {
        let mut builder = reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .connect_timeout(Self::DEFAULT_CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .http1_only()
            .user_agent(Self::USER_AGENT)
            .no_proxy();

        if let Some(destination) = destination {
            builder = builder.resolve_to_addrs(&destination.host, &destination.addrs);
        }

        builder
            .build()
            .map_err(|error| format!("failed to build webhook HTTP transport: {error}"))
    }
}

impl WebhookTransport for HttpWebhookTransport {
    fn send(&self, request: &WebhookRequest) -> TransportResult {
        let destination = if self.allow_private_destinations {
            None
        } else {
            match validate_public_http_destination(&request.url, "webhook") {
                Ok(destination) => Some(destination),
                Err(error) => return TransportResult::RequestError(error),
            }
        };
        let client = match self.client_for_destination(destination.as_ref()) {
            Ok(client) => client,
            Err(error) => return TransportResult::RequestError(error),
        };
        let url = destination
            .as_ref()
            .map_or(request.url.as_str(), |destination| destination.url.as_str());
        let mut builder = client.post(url).body(request.body.clone());
        for (name, value) in request.headers.as_pairs() {
            builder = builder.header(name, value);
        }

        match builder.send() {
            Ok(response) => TransportResult::HttpStatus(response.status().as_u16()),
            Err(error) => TransportResult::RequestError(error.to_string()),
        }
    }
}

/// Effect sink that turns ingest webhook effects into ledger rows and jobs.
pub struct WebhookEnqueueEffectSink {
    store: SharedStore,
    scheduler: Arc<dyn WebhookScheduler>,
    cooldown: Arc<dyn WebhookCooldown>,
}

impl WebhookEnqueueEffectSink {
    /// Build a webhook enqueue sink from explicit runtime boundaries.
    pub fn new(
        store: SharedStore,
        scheduler: Arc<dyn WebhookScheduler>,
        cooldown: Arc<dyn WebhookCooldown>,
    ) -> Self {
        Self {
            store,
            scheduler,
            cooldown,
        }
    }
}

impl IngestEffectSink for WebhookEnqueueEffectSink {
    fn handle(&self, effects: &[IngestEffect]) -> Result<(), String> {
        let mut errors = Vec::new();
        for effect in effects {
            if let IngestEffect::EnqueueWebhook {
                event,
                payload_json,
            } = effect
                && let Err(error) = self.enqueue_event(event, payload_json)
            {
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

impl WebhookEnqueueEffectSink {
    /// Enqueue one already-recorded service event for matching webhook subscribers.
    pub fn enqueue_event(&self, event: &str, payload_json: &str) -> Result<(), String> {
        let payload = serde_json::from_str(payload_json)
            .map_err(|error| format!("invalid webhook payload: {error}"))?;
        let authority = WebhookEventAuthority::from_payload(&payload);
        let now = current_rfc3339();
        let subscriptions = {
            let store = self.store.lock();
            store
                .active_webhook_subscriptions_for_event_scoped(
                    event,
                    &authority.tenant_id,
                    &authority.project_id,
                    authority.service.as_deref(),
                )
                .map_err(|error| error.to_string())?
        };
        let endpoints = subscriptions.into_iter().map(endpoint_from_subscription);
        let decisions = plan_enqueue_for_event(
            event,
            &payload,
            endpoints,
            || canary_core::ids::DeliveryId::generate().into_string(),
            |key| self.cooldown.in_cooldown(key),
        );

        for decision in decisions {
            match decision {
                WebhookEnqueueDecision::Schedule {
                    delivery,
                    job,
                    cooldown_key,
                } => {
                    self.create_pending(delivery, &authority, &now)?;
                    match self.scheduler.schedule(&job) {
                        Ok(()) => self.cooldown.mark(&cooldown_key),
                        Err(error) => {
                            self.discard(&job, "enqueue_failed", &now)?;
                            return Err(format!("failed to schedule webhook: {error}"));
                        }
                    }
                }
                WebhookEnqueueDecision::Suppress { delivery, reason } => {
                    self.create_suppressed(delivery, &authority, &reason, &now)?;
                }
            }
        }

        Ok(())
    }

    fn create_pending(
        &self,
        delivery: canary_workers::webhooks::PlannedWebhookDelivery,
        authority: &WebhookEventAuthority,
        now: &str,
    ) -> Result<(), String> {
        let mut store = self.store.lock();
        store
            .create_pending_webhook_delivery(delivery_insert(delivery, authority, now))
            .map_err(|error| error.to_string())
    }

    fn create_suppressed(
        &self,
        delivery: canary_workers::webhooks::PlannedWebhookDelivery,
        authority: &WebhookEventAuthority,
        reason: &str,
        now: &str,
    ) -> Result<(), String> {
        let mut store = self.store.lock();
        store
            .create_suppressed_webhook_delivery(delivery_insert(delivery, authority, now), reason)
            .map_err(|error| error.to_string())
    }

    fn discard(&self, job: &WebhookJob, reason: &str, now: &str) -> Result<(), String> {
        let Some(delivery_id) = job.delivery_id.as_deref() else {
            return Ok(());
        };
        let mut store = self.store.lock();
        store
            .mark_webhook_delivery_discarded(delivery_id, reason, now)
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebhookEventAuthority {
    tenant_id: String,
    project_id: String,
    service: Option<String>,
}

impl WebhookEventAuthority {
    pub(crate) fn from_payload(payload: &Value) -> Self {
        Self {
            tenant_id: payload
                .get("tenant_id")
                .and_then(Value::as_str)
                .unwrap_or(BOOTSTRAP_TENANT_ID)
                .to_owned(),
            project_id: payload
                .get("project_id")
                .and_then(Value::as_str)
                .unwrap_or(BOOTSTRAP_PROJECT_ID)
                .to_owned(),
            service: payload_service(payload).map(ToOwned::to_owned),
        }
    }
}

fn payload_service(payload: &Value) -> Option<&str> {
    payload
        .get("error")
        .and_then(|error| error.get("service"))
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("incident")
                .and_then(|incident| incident.get("service"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            payload
                .get("target")
                .and_then(|target| target.get("service"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            payload
                .get("monitor")
                .and_then(|monitor| monitor.get("service"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            payload
                .get("annotation")
                .and_then(|annotation| annotation.get("service"))
                .and_then(Value::as_str)
        })
        .or_else(|| payload.get("service").and_then(Value::as_str))
}

pub(crate) fn endpoint_from_subscription(subscription: WebhookSubscription) -> WebhookEndpoint {
    WebhookEndpoint {
        id: subscription.id,
        url: subscription.url,
        secret: subscription.secret,
        active: subscription.active,
    }
}

pub(crate) fn delivery_insert(
    delivery: canary_workers::webhooks::PlannedWebhookDelivery,
    authority: &WebhookEventAuthority,
    now: &str,
) -> WebhookDeliveryInsert {
    WebhookDeliveryInsert {
        delivery_id: delivery.delivery_id,
        webhook_id: delivery.webhook_id,
        tenant_id: authority.tenant_id.clone(),
        project_id: authority.project_id.clone(),
        service: authority.service.clone(),
        event: delivery.event,
        now: now.to_owned(),
    }
}

fn job_args(job: &WebhookJob) -> Value {
    let mut args = json!({
        "webhook_id": job.webhook_id,
        "payload": job.payload,
        "event": job.event,
    });

    if let Some(delivery_id) = &job.delivery_id {
        args["delivery_id"] = Value::String(delivery_id.clone());
    }

    args
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration as StdDuration};

    use super::*;

    #[test]
    fn in_memory_cooldown_suppresses_until_ttl_expires() {
        let cooldown = InMemoryWebhookCooldown::with_ttl(StdDuration::from_millis(20));

        assert!(!cooldown.in_cooldown("WHK-a:error.new_class:grp-a"));
        cooldown.mark("WHK-a:error.new_class:grp-a");
        assert!(cooldown.in_cooldown("WHK-a:error.new_class:grp-a"));
        assert!(!cooldown.in_cooldown("WHK-a:error.new_class:grp-b"));

        thread::sleep(StdDuration::from_millis(25));
        assert!(!cooldown.in_cooldown("WHK-a:error.new_class:grp-a"));
    }

    #[test]
    fn webhook_event_authority_extracts_service_from_event_payload_families() {
        for (payload, expected_service) in [
            (json!({"error": {"service": "errors"}}), "errors"),
            (json!({"incident": {"service": "incidents"}}), "incidents"),
            (json!({"target": {"service": "targets"}}), "targets"),
            (json!({"monitor": {"service": "monitors"}}), "monitors"),
            (
                json!({"annotation": {"service": "annotations"}}),
                "annotations",
            ),
            (json!({"service": "top-level"}), "top-level"),
        ] {
            assert_eq!(
                WebhookEventAuthority::from_payload(&payload)
                    .service
                    .as_deref(),
                Some(expected_service)
            );
        }
    }
}
