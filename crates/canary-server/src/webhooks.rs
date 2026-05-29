use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration as StdDuration, Instant},
};

use canary_ingest::IngestEffect;
use canary_store::{
    Store, WebhookDeliveryInsert, WebhookDeliveryJobCompletion, WebhookDeliveryJobInsert,
    WebhookDeliveryJobRow, WebhookSubscription,
};
use canary_workers::webhooks::{
    CircuitDecision, CircuitEffect, DeliveryExecution, DeliveryLedgerAction, DeliveryOutcome,
    MAX_ATTEMPTS, TransportResult, WebhookEndpoint, WebhookEnqueueDecision, WebhookJob,
    WebhookLookup, WebhookRequest, plan_enqueue_for_event, try_execute_delivery,
};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::IngestEffectSink;

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

/// Runtime boundary for webhook circuit state.
pub trait WebhookCircuit: Send + Sync + 'static {
    /// Return the circuit decision before one delivery attempt.
    fn decision(&self, webhook_id: &str) -> CircuitDecision;

    /// Record a successful delivery.
    fn record_success(&self, webhook_id: &str);

    /// Record a failed delivery.
    fn record_failure(&self, webhook_id: &str);
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

/// In-process per-webhook delivery circuit breaker.
///
/// Phoenix opens a circuit after ten consecutive delivery failures and allows a
/// probe after five minutes. This adapter keeps that policy out of the delivery
/// planner while making the runtime state explicit and testable.
#[derive(Debug)]
pub struct InMemoryWebhookCircuit {
    failure_threshold: u32,
    probe_interval: StdDuration,
    states: Mutex<HashMap<String, WebhookCircuitState>>,
}

#[derive(Debug, Clone)]
struct WebhookCircuitState {
    failures: u32,
    last_failure_at: Instant,
}

impl InMemoryWebhookCircuit {
    const DEFAULT_FAILURE_THRESHOLD: u32 = 10;
    const DEFAULT_PROBE_INTERVAL: StdDuration = StdDuration::from_secs(5 * 60);

    /// Build the Phoenix-compatible webhook circuit breaker.
    pub fn phoenix_default() -> Self {
        Self::with_policy(
            Self::DEFAULT_FAILURE_THRESHOLD,
            Self::DEFAULT_PROBE_INTERVAL,
        )
    }

    /// Build circuit state with an explicit threshold and probe interval for tests.
    pub fn with_policy(failure_threshold: u32, probe_interval: StdDuration) -> Self {
        Self {
            failure_threshold,
            probe_interval,
            states: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryWebhookCircuit {
    fn default() -> Self {
        Self::phoenix_default()
    }
}

impl WebhookCircuit for InMemoryWebhookCircuit {
    fn decision(&self, webhook_id: &str) -> CircuitDecision {
        let Ok(states) = self.states.lock() else {
            return CircuitDecision::Closed;
        };
        let Some(state) = states.get(webhook_id) else {
            return CircuitDecision::Closed;
        };
        if state.failures < self.failure_threshold {
            return CircuitDecision::Closed;
        }
        if state.last_failure_at.elapsed() >= self.probe_interval {
            CircuitDecision::Probe
        } else {
            CircuitDecision::Open
        }
    }

    fn record_success(&self, webhook_id: &str) {
        let Ok(mut states) = self.states.lock() else {
            return;
        };
        states.remove(webhook_id);
    }

    fn record_failure(&self, webhook_id: &str) {
        let Ok(mut states) = self.states.lock() else {
            return;
        };
        let state = states
            .entry(webhook_id.to_owned())
            .or_insert_with(|| WebhookCircuitState {
                failures: 0,
                last_failure_at: Instant::now(),
            });
        state.failures = state.failures.saturating_add(1);
        state.last_failure_at = Instant::now();
    }
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

/// Sequential scheduled-job drain for webhook delivery jobs.
pub struct WebhookDeliveryDrain {
    store: Arc<Mutex<Store>>,
    runtime: WebhookDeliveryRuntime,
    max_jobs: u32,
}

/// Summary of one drain pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookDeliveryDrainReport {
    /// Jobs claimed from the scheduler store.
    pub claimed: u32,
    /// Jobs completed after successful delivery or intentional skip.
    pub completed: u32,
    /// Jobs rescheduled for retry.
    pub retried: u32,
    /// Jobs permanently discarded by the scheduler.
    pub discarded: u32,
}

/// Dedicated OS-thread runner for scheduled webhook delivery drains.
///
/// `HttpWebhookTransport` is intentionally blocking. This worker keeps that
/// blocking path out of Axum request tasks while reusing the bounded sequential
/// `WebhookDeliveryDrain` instead of introducing a generic job framework.
pub struct WebhookDeliveryDrainWorker {
    stop: Arc<DrainStop>,
    handle: Option<JoinHandle<()>>,
}

impl WebhookDeliveryDrainWorker {
    /// Spawn one named background thread that drains immediately, then on the interval.
    pub fn spawn(drain: WebhookDeliveryDrain, interval: StdDuration) -> Result<Self, String> {
        if interval.is_zero() {
            return Err("webhook drain interval must be greater than zero".to_owned());
        }

        let stop = Arc::new(DrainStop::default());
        let thread_stop = stop.clone();
        let handle = thread::Builder::new()
            .name("canary-webhook-drain".to_owned())
            .spawn(move || run_drain_worker(drain, interval, thread_stop))
            .map_err(|error| format!("failed to spawn webhook drain worker: {error}"))?;

        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }

    /// Request shutdown without waiting for an in-flight drain pass to finish.
    pub fn stop(&self) {
        self.stop.request();
    }

    /// Request shutdown and wait for the worker thread to exit.
    pub fn join(mut self) -> Result<(), String> {
        self.stop();
        self.join_handle()
    }

    fn join_handle(&mut self) -> Result<(), String> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        match handle.join() {
            Ok(()) => Ok(()),
            Err(_) => Err("webhook drain worker panicked".to_owned()),
        }
    }
}

impl Drop for WebhookDeliveryDrainWorker {
    fn drop(&mut self) {
        self.stop.request();
        let _ = self.join_handle();
    }
}

impl WebhookDeliveryDrain {
    /// Build a drain with an explicit maximum number of jobs per pass.
    pub fn new(store: Arc<Mutex<Store>>, runtime: WebhookDeliveryRuntime, max_jobs: u32) -> Self {
        Self {
            store,
            runtime,
            max_jobs,
        }
    }

    /// Claim due jobs, execute them sequentially, and persist retry/terminal state.
    pub fn drain_due(&self, now: &str) -> Result<WebhookDeliveryDrainReport, String> {
        let jobs = {
            let mut store = self
                .store
                .lock()
                .map_err(|_| "store lock poisoned".to_owned())?;
            store
                .claim_due_webhook_delivery_jobs(now, self.max_jobs)
                .map_err(|error| error.to_string())?
        };

        let mut report = WebhookDeliveryDrainReport {
            claimed: jobs.len() as u32,
            ..WebhookDeliveryDrainReport::default()
        };

        for row in jobs {
            let job = match job_from_row(&row) {
                Ok(job) => job,
                Err(_) => {
                    self.complete_job(
                        row.id,
                        WebhookDeliveryJobCompletion::Discard {
                            now: now.to_owned(),
                        },
                    )?;
                    report.discarded += 1;
                    continue;
                }
            };

            let execution = self.runtime.deliver(&job)?;
            match completion_for_execution(now, &execution)? {
                DrainCompletion::Retry { scheduled_at } => {
                    self.complete_job(
                        row.id,
                        WebhookDeliveryJobCompletion::Retry { scheduled_at },
                    )?;
                    report.retried += 1;
                }
                DrainCompletion::Complete => {
                    self.complete_job(
                        row.id,
                        WebhookDeliveryJobCompletion::Complete {
                            now: now.to_owned(),
                        },
                    )?;
                    report.completed += 1;
                }
                DrainCompletion::Discard => {
                    self.complete_job(
                        row.id,
                        WebhookDeliveryJobCompletion::Discard {
                            now: now.to_owned(),
                        },
                    )?;
                    report.discarded += 1;
                }
            }
        }

        Ok(report)
    }

    fn complete_job(
        &self,
        job_id: i64,
        completion: WebhookDeliveryJobCompletion,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        store
            .complete_webhook_delivery_job(job_id, completion)
            .map_err(|error| error.to_string())
    }
}

#[derive(Default)]
struct DrainStop {
    stopping: AtomicBool,
    lock: Mutex<()>,
    condvar: Condvar,
}

impl DrainStop {
    fn request(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.condvar.notify_all();
    }

    fn is_requested(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    fn wait(&self, interval: StdDuration) -> bool {
        if self.is_requested() {
            return true;
        }

        let Ok(guard) = self.lock.lock() else {
            return true;
        };
        let _ = self
            .condvar
            .wait_timeout_while(guard, interval, |_| !self.stopping.load(Ordering::SeqCst));
        self.is_requested()
    }
}

fn run_drain_worker(drain: WebhookDeliveryDrain, interval: StdDuration, stop: Arc<DrainStop>) {
    while !stop.is_requested() {
        let _ = catch_unwind(AssertUnwindSafe(|| drain.drain_due(&current_rfc3339())));
        if stop.wait(interval) {
            break;
        }
    }
}

enum DrainCompletion {
    Retry { scheduled_at: String },
    Complete,
    Discard,
}

fn completion_for_execution(
    now: &str,
    execution: &DeliveryExecution,
) -> Result<DrainCompletion, String> {
    if let Some(retry_after_seconds) = execution.retry_after_seconds {
        return Ok(DrainCompletion::Retry {
            scheduled_at: add_seconds(now, retry_after_seconds)?,
        });
    }

    Ok(match &execution.outcome {
        DeliveryOutcome::Delivered | DeliveryOutcome::Suppressed { .. } => {
            DrainCompletion::Complete
        }
        DeliveryOutcome::Discarded { reason } if is_scheduler_discard(reason) => {
            DrainCompletion::Discard
        }
        DeliveryOutcome::Discarded { .. } => DrainCompletion::Complete,
        DeliveryOutcome::Retry { .. } => DrainCompletion::Complete,
    })
}

fn is_scheduler_discard(reason: &str) -> bool {
    reason == "request_error" || reason.starts_with("http_")
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

fn job_from_row(row: &WebhookDeliveryJobRow) -> Result<WebhookJob, String> {
    let args = row
        .args
        .as_object()
        .ok_or_else(|| "webhook job args must be a JSON object".to_owned())?;
    let webhook_id = required_string(args.get("webhook_id"), "webhook_id")?;
    let event = required_string(args.get("event"), "event")?;
    let payload = args
        .get("payload")
        .cloned()
        .ok_or_else(|| "webhook job args missing payload".to_owned())?;
    let delivery_id = args
        .get("delivery_id")
        .and_then(Value::as_str)
        .map(str::to_owned);

    Ok(WebhookJob {
        webhook_id,
        payload,
        event,
        delivery_id,
        legacy_job_id: Some(row.id),
        attempt: row.attempt,
        max_attempts: if row.max_attempts == 0 {
            MAX_ATTEMPTS
        } else {
            row.max_attempts
        },
    })
}

fn required_string(value: Option<&Value>, field: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("webhook job args missing {field}"))
}

fn add_seconds(now: &str, seconds: u64) -> Result<String, String> {
    let now = OffsetDateTime::parse(now, &Rfc3339)
        .map_err(|error| format!("invalid drain timestamp: {error}"))?;
    now.checked_add(Duration::seconds(seconds as i64))
        .ok_or_else(|| "retry timestamp overflow".to_owned())?
        .format(&Rfc3339)
        .map_err(|error| format!("failed to format retry timestamp: {error}"))
}

fn current_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
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
    fn in_memory_circuit_opens_probes_and_resets_on_success() {
        let circuit = InMemoryWebhookCircuit::with_policy(2, StdDuration::from_millis(20));

        assert_eq!(circuit.decision("WHK-a"), CircuitDecision::Closed);
        circuit.record_failure("WHK-a");
        assert_eq!(circuit.decision("WHK-a"), CircuitDecision::Closed);
        circuit.record_failure("WHK-a");
        assert_eq!(circuit.decision("WHK-a"), CircuitDecision::Open);

        thread::sleep(StdDuration::from_millis(25));
        assert_eq!(circuit.decision("WHK-a"), CircuitDecision::Probe);

        circuit.record_success("WHK-a");
        assert_eq!(circuit.decision("WHK-a"), CircuitDecision::Closed);
    }

    #[test]
    fn in_memory_circuit_failed_probe_reopens_for_another_probe_interval() {
        let circuit = InMemoryWebhookCircuit::with_policy(1, StdDuration::from_millis(20));

        circuit.record_failure("WHK-a");
        thread::sleep(StdDuration::from_millis(25));
        assert_eq!(circuit.decision("WHK-a"), CircuitDecision::Probe);

        circuit.record_failure("WHK-a");
        assert_eq!(circuit.decision("WHK-a"), CircuitDecision::Open);
    }
}
