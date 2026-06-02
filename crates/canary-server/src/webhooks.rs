use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration as StdDuration, Instant},
};

use canary_ingest::IngestEffect;
use canary_store::{Store, WebhookDeliveryInsert, WebhookDeliveryJobInsert, WebhookSubscription};
use canary_workers::webhooks::{
    TransportResult, WebhookEndpoint, WebhookEnqueueDecision, WebhookJob, WebhookRequest,
    plan_enqueue_for_event,
};
use serde_json::{Value, json};

use crate::{IngestEffectSink, current_rfc3339};

/// Runtime boundary for scheduling webhook delivery jobs.
pub trait WebhookScheduler: Send + Sync + 'static {
    /// Schedule one webhook job after its pending ledger row has been created.
    fn schedule(&self, job: &WebhookJob) -> Result<(), String>;
}

/// Store-backed scheduler for webhook delivery jobs.
pub struct StoreWebhookScheduler {
    store: Arc<Mutex<Store>>,
}

impl StoreWebhookScheduler {
    /// Build a scheduler backed by the shared single-writer store.
    pub fn new(store: Arc<Mutex<Store>>) -> Self {
        Self { store }
    }
}

impl WebhookScheduler for StoreWebhookScheduler {
    fn schedule(&self, job: &WebhookJob) -> Result<(), String> {
        let now = current_rfc3339();
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
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
/// Phoenix stores this in ETS. The Rust server keeps the same process-local
/// contract explicitly here: cooldown state is advisory, monotonic, and lost on
/// restart.
#[derive(Debug)]
pub struct InMemoryWebhookCooldown {
    ttl: StdDuration,
    marked_at: Mutex<HashMap<String, Instant>>,
}

impl InMemoryWebhookCooldown {
    const DEFAULT_TTL: StdDuration = StdDuration::from_secs(5 * 60);

    /// Build the Phoenix-compatible five-minute webhook cooldown.
    pub fn phoenix_default() -> Self {
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
        Self::phoenix_default()
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
    client: reqwest::blocking::Client,
}

impl HttpWebhookTransport {
    const DEFAULT_TIMEOUT: StdDuration = StdDuration::from_secs(10);
    const DEFAULT_CONNECT_TIMEOUT: StdDuration = StdDuration::from_secs(3);
    const USER_AGENT: &'static str = concat!("canary-server/", env!("CARGO_PKG_VERSION"));

    /// Build an HTTP transport with Phoenix-compatible timeout and no redirects.
    ///
    /// TLS certificate validation stays enabled. The blocking send path is meant
    /// for the webhook drain worker, not an Axum request task.
    pub fn try_new() -> Result<Self, String> {
        Self::with_timeout(Self::DEFAULT_TIMEOUT)
    }

    /// Build an HTTP transport with an explicit timeout and no hidden retries.
    pub fn with_timeout(timeout: StdDuration) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(Self::DEFAULT_CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .http1_only()
            .user_agent(Self::USER_AGENT)
            .build()
            .map_err(|error| format!("failed to build webhook HTTP transport: {error}"))?;

        Ok(Self { client })
    }
}

impl WebhookTransport for HttpWebhookTransport {
    fn send(&self, request: &WebhookRequest) -> TransportResult {
        let mut builder = self.client.post(&request.url).body(request.body.clone());
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
    store: Arc<Mutex<Store>>,
    scheduler: Arc<dyn WebhookScheduler>,
    cooldown: Arc<dyn WebhookCooldown>,
}

impl WebhookEnqueueEffectSink {
    /// Build a webhook enqueue sink from explicit runtime boundaries.
    pub fn new(
        store: Arc<Mutex<Store>>,
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
        let now = current_rfc3339();
        let subscriptions = {
            let store = self
                .store
                .lock()
                .map_err(|_| "store lock poisoned".to_owned())?;
            store
                .active_webhook_subscriptions_for_event(event)
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
                    self.create_pending(delivery, &now)?;
                    match self.scheduler.schedule(&job) {
                        Ok(()) => self.cooldown.mark(&cooldown_key),
                        Err(error) => {
                            self.discard(&job, "enqueue_failed", &now)?;
                            return Err(format!("failed to schedule webhook: {error}"));
                        }
                    }
                }
                WebhookEnqueueDecision::Suppress { delivery, reason } => {
                    self.create_suppressed(delivery, &reason, &now)?;
                }
            }
        }

        Ok(())
    }

    fn create_pending(
        &self,
        delivery: canary_workers::webhooks::PlannedWebhookDelivery,
        now: &str,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        store
            .create_pending_webhook_delivery(delivery_insert(delivery, now))
            .map_err(|error| error.to_string())
    }

    fn create_suppressed(
        &self,
        delivery: canary_workers::webhooks::PlannedWebhookDelivery,
        reason: &str,
        now: &str,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        store
            .create_suppressed_webhook_delivery(delivery_insert(delivery, now), reason)
            .map_err(|error| error.to_string())
    }

    fn discard(&self, job: &WebhookJob, reason: &str, now: &str) -> Result<(), String> {
        let Some(delivery_id) = job.delivery_id.as_deref() else {
            return Ok(());
        };
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        store
            .mark_webhook_delivery_discarded(delivery_id, reason, now)
            .map_err(|error| error.to_string())
    }
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
    now: &str,
) -> WebhookDeliveryInsert {
    WebhookDeliveryInsert {
        delivery_id: delivery.delivery_id,
        webhook_id: delivery.webhook_id,
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
}
