use std::sync::{Arc, Mutex};

use canary_ingest::IngestEffect;
use canary_store::{Store, WebhookDeliveryInsert, WebhookSubscription};
use canary_workers::webhooks::{
    CircuitDecision, CircuitEffect, DeliveryExecution, DeliveryLedgerAction, TransportResult,
    WebhookEndpoint, WebhookEnqueueDecision, WebhookJob, WebhookLookup, WebhookRequest,
    plan_enqueue_for_event, try_execute_delivery,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::IngestEffectSink;

/// Runtime boundary for scheduling webhook delivery jobs.
pub trait WebhookScheduler: Send + Sync + 'static {
    /// Schedule one webhook job after its pending ledger row has been created.
    fn schedule(&self, job: &WebhookJob) -> Result<(), String>;
}

/// Runtime boundary for webhook cooldown state.
pub trait WebhookCooldown: Send + Sync + 'static {
    /// Return true when the event should be suppressed.
    fn in_cooldown(&self, key: &str) -> bool;

    /// Mark a key after the scheduler accepts a job.
    fn mark(&self, key: &str);
}

/// Runtime boundary for outbound webhook transport.
pub trait WebhookTransport: Send + Sync + 'static {
    /// Send one signed webhook request.
    fn send(&self, request: &WebhookRequest) -> TransportResult;
}

/// Runtime boundary for webhook circuit state.
pub trait WebhookCircuit: Send + Sync + 'static {
    /// Return the circuit decision before one delivery attempt.
    fn decision(&self, webhook_id: &str) -> CircuitDecision;

    /// Record a successful delivery.
    fn record_success(&self, webhook_id: &str);

    /// Record a failed delivery.
    fn record_failure(&self, webhook_id: &str);
}

#[derive(Debug, Default)]
pub(crate) struct NoopWebhookCooldown;

impl WebhookCooldown for NoopWebhookCooldown {
    fn in_cooldown(&self, _key: &str) -> bool {
        false
    }

    fn mark(&self, _key: &str) {}
}

#[derive(Debug, Default)]
struct NoopWebhookCircuit;

impl WebhookCircuit for NoopWebhookCircuit {
    fn decision(&self, _webhook_id: &str) -> CircuitDecision {
        CircuitDecision::Closed
    }

    fn record_success(&self, _webhook_id: &str) {}

    fn record_failure(&self, _webhook_id: &str) {}
}

/// Runtime adapter for executing one scheduled webhook delivery job.
pub struct WebhookDeliveryRuntime {
    store: Arc<Mutex<Store>>,
    transport: Arc<dyn WebhookTransport>,
    circuit: Arc<dyn WebhookCircuit>,
}

impl WebhookDeliveryRuntime {
    /// Build a delivery runtime from explicit side-effect boundaries.
    pub fn new(
        store: Arc<Mutex<Store>>,
        transport: Arc<dyn WebhookTransport>,
        circuit: Arc<dyn WebhookCircuit>,
    ) -> Self {
        Self {
            store,
            transport,
            circuit,
        }
    }

    /// Build a delivery runtime with a closed no-op circuit.
    pub fn new_without_circuit(
        store: Arc<Mutex<Store>>,
        transport: Arc<dyn WebhookTransport>,
    ) -> Self {
        Self::new(store, transport, Arc::new(NoopWebhookCircuit))
    }

    /// Execute one scheduled job and persist the ordered ledger actions.
    pub fn deliver(&self, job: &WebhookJob) -> Result<DeliveryExecution, String> {
        let lookup = self.lookup_webhook(job)?;
        let circuit = match &lookup {
            WebhookLookup::Active(endpoint) => self.circuit.decision(&endpoint.id),
            WebhookLookup::Missing | WebhookLookup::Inactive(_) => CircuitDecision::Closed,
        };

        let execution = try_execute_delivery(
            job,
            lookup,
            circuit,
            |action| self.apply_ledger_action(action),
            |request| self.transport.send(&request),
        )?;
        self.apply_circuit_effect(&execution.circuit_effect);

        Ok(execution)
    }

    fn lookup_webhook(&self, job: &WebhookJob) -> Result<WebhookLookup, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        let subscription = store
            .webhook_subscription(&job.webhook_id)
            .map_err(|error| error.to_string())?;

        Ok(match subscription.map(endpoint_from_subscription) {
            None => WebhookLookup::Missing,
            Some(endpoint) if endpoint.active => WebhookLookup::Active(endpoint),
            Some(endpoint) => WebhookLookup::Inactive(endpoint),
        })
    }

    fn apply_ledger_action(&self, action: DeliveryLedgerAction) -> Result<(), String> {
        let now = current_rfc3339();
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;

        match action {
            DeliveryLedgerAction::CreatePending(delivery) => store
                .create_pending_webhook_delivery(delivery_insert(delivery, &now))
                .map_err(|error| error.to_string()),
            DeliveryLedgerAction::MarkAttempt { delivery_id } => store
                .mark_webhook_delivery_attempt(&delivery_id, &now)
                .map_err(|error| error.to_string()),
            DeliveryLedgerAction::MarkDelivered { delivery_id } => store
                .mark_webhook_delivery_delivered(&delivery_id, &now)
                .map_err(|error| error.to_string()),
            DeliveryLedgerAction::MarkDiscarded {
                delivery_id,
                reason,
            } => store
                .mark_webhook_delivery_discarded(&delivery_id, &reason, &now)
                .map_err(|error| error.to_string()),
            DeliveryLedgerAction::CreateSuppressed { delivery, reason } => store
                .create_suppressed_webhook_delivery(delivery_insert(delivery, &now), &reason)
                .map_err(|error| error.to_string()),
        }
    }

    fn apply_circuit_effect(&self, effect: &CircuitEffect) {
        match effect {
            CircuitEffect::None => {}
            CircuitEffect::RecordSuccess { webhook_id } => self.circuit.record_success(webhook_id),
            CircuitEffect::RecordFailure { webhook_id } => self.circuit.record_failure(webhook_id),
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
    fn enqueue_event(&self, event: &str, payload_json: &str) -> Result<(), String> {
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

fn endpoint_from_subscription(subscription: WebhookSubscription) -> WebhookEndpoint {
    WebhookEndpoint {
        id: subscription.id,
        url: subscription.url,
        secret: subscription.secret,
        active: subscription.active,
    }
}

fn delivery_insert(
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

fn current_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
