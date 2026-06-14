//! Scheduled webhook delivery runtime and drain policy.
//!
//! Enqueue planning and HTTP transport live in `webhooks.rs`. This module owns
//! the delivery side: circuit state, lookup/execute/ledger application, retry
//! scheduling, and the dedicated blocking drain worker.

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

use canary_store::{Store, WebhookDeliveryJobCompletion, WebhookDeliveryJobRow};
use canary_workers::webhooks::{
    CircuitDecision, CircuitEffect, DeliveryExecution, DeliveryLedgerAction, DeliveryOutcome,
    MAX_ATTEMPTS, WebhookJob, WebhookLookup, try_execute_delivery,
};
use serde_json::Value;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    WorkerHealthHandle, WorkerName, WorkerPressureSnapshot,
    server_time::{current_rfc3339, current_unix_millis},
    webhooks::{
        WebhookEventAuthority, WebhookTransport, delivery_insert, endpoint_from_subscription,
    },
};

const WEBHOOK_EXECUTION_LEASE_SECONDS: u64 = 60;

/// Runtime boundary for webhook circuit state.
pub trait WebhookCircuit: Send + Sync + 'static {
    /// Return the circuit decision before one delivery attempt.
    fn decision(&self, webhook_id: &str) -> CircuitDecision;

    /// Record a successful delivery.
    fn record_success(&self, webhook_id: &str);

    /// Record a failed delivery.
    fn record_failure(&self, webhook_id: &str);
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
        let mut job = job.clone();
        job.attempt_timestamp = Some(current_rfc3339());

        let lookup = self.lookup_webhook(&job)?;
        let circuit = match &lookup {
            WebhookLookup::Active(endpoint) => self.circuit.decision(&endpoint.id),
            WebhookLookup::Missing | WebhookLookup::Inactive(_) => CircuitDecision::Closed,
        };
        let authority = WebhookEventAuthority::from_payload(&job.payload);

        let execution = try_execute_delivery(
            &job,
            lookup,
            circuit,
            |action| self.apply_ledger_action(action, &authority),
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

    fn apply_ledger_action(
        &self,
        action: DeliveryLedgerAction,
        authority: &WebhookEventAuthority,
    ) -> Result<(), String> {
        let now = current_rfc3339();
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;

        match action {
            DeliveryLedgerAction::CreatePending(delivery) => store
                .create_pending_webhook_delivery(delivery_insert(delivery, authority, &now))
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
                .create_suppressed_webhook_delivery(
                    delivery_insert(delivery, authority, &now),
                    &reason,
                )
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
    /// Due scheduler rows observed after stale recovery and before claiming.
    pub due_count: u32,
    /// Executing scheduler rows observed after stale recovery and before claiming.
    pub in_flight_count: u32,
    /// Age in milliseconds of the oldest due scheduler row.
    pub oldest_due_age_ms: Option<u64>,
    /// Stale executing jobs recovered before claiming due work.
    pub recovered: u32,
    /// Stale executing jobs leased back to retry.
    pub recovery_retried: u32,
    /// Stale executing jobs discarded after exhausting attempts.
    pub recovery_discarded: u32,
    /// Jobs claimed from the scheduler store.
    pub claimed: u32,
    /// Jobs completed after successful delivery or intentional skip.
    pub completed: u32,
    /// Jobs completed because the webhook circuit was open.
    pub circuit_open_suppressed: u32,
    /// Jobs rescheduled for retry.
    pub retried: u32,
    /// Jobs permanently discarded by the scheduler.
    pub discarded: u32,
}

/// Dedicated OS-thread runner for scheduled webhook delivery drains.
///
/// `WebhookTransport` may block. This worker keeps that path out of Axum
/// request tasks while reusing the bounded sequential `WebhookDeliveryDrain`
/// instead of introducing a generic job framework.
pub struct WebhookDeliveryDrainWorker {
    stop: Arc<DrainStop>,
    health: WorkerHealthHandle,
    handle: Option<JoinHandle<()>>,
}

impl WebhookDeliveryDrainWorker {
    /// Spawn one named background thread that drains immediately, then on the interval.
    pub fn spawn(drain: WebhookDeliveryDrain, interval: StdDuration) -> Result<Self, String> {
        Self::spawn_with_health(
            drain,
            interval,
            WorkerHealthHandle::new(WorkerName::WebhookDelivery),
        )
    }

    /// Spawn one named background thread with an explicit health recorder.
    pub(crate) fn spawn_with_health(
        drain: WebhookDeliveryDrain,
        interval: StdDuration,
        health: WorkerHealthHandle,
    ) -> Result<Self, String> {
        if interval.is_zero() {
            return Err("webhook drain interval must be greater than zero".to_owned());
        }

        let stop = Arc::new(DrainStop::default());
        let thread_stop = stop.clone();
        health.mark_started();
        let thread_health = health.clone();
        let handle = thread::Builder::new()
            .name("canary-webhook-drain".to_owned())
            .spawn(move || run_drain_worker(drain, interval, thread_stop, thread_health))
            .map_err(|error| format!("failed to spawn webhook drain worker: {error}"))?;

        Ok(Self {
            stop,
            health,
            handle: Some(handle),
        })
    }

    /// Request shutdown without waiting for an in-flight drain pass to finish.
    pub fn stop(&self) {
        self.stop.request();
    }

    /// Return the visible runtime failure count.
    pub fn failure_count(&self) -> u64 {
        self.health.snapshot().failure_count
    }

    /// Return the readiness-visible worker health snapshot.
    pub fn health_snapshot(&self) -> canary_http::public::WorkerReadyzCheck {
        self.health.snapshot()
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
        validate_drain_timestamp(now)?;
        let stale_before = subtract_seconds(now, WEBHOOK_EXECUTION_LEASE_SECONDS)?;

        let jobs = {
            let mut store = self
                .store
                .lock()
                .map_err(|_| "store lock poisoned".to_owned())?;
            let recovery = store
                .recover_stale_webhook_delivery_jobs(now, &stale_before, self.max_jobs)
                .map_err(|error| error.to_string())?;
            let due_summary = store
                .webhook_delivery_due_summary(now)
                .map_err(|error| error.to_string())?;
            store
                .claim_due_webhook_delivery_jobs(now, self.max_jobs)
                .map_err(|error| error.to_string())
                .map(|jobs| (recovery, due_summary, jobs))?
        };
        let (recovery, due_summary, jobs) = jobs;

        let mut report = WebhookDeliveryDrainReport {
            due_count: due_summary.due_count,
            in_flight_count: due_summary.in_flight_count,
            oldest_due_age_ms: oldest_due_age_ms(now, due_summary.oldest_scheduled_at.as_deref())?,
            recovered: recovery.recovered,
            recovery_retried: recovery.retried,
            recovery_discarded: recovery.discarded,
            claimed: jobs.len() as u32,
            ..WebhookDeliveryDrainReport::default()
        };

        for row in jobs {
            let job = match job_from_row(&row) {
                Ok(job) => job,
                Err(_) => {
                    self.complete_job(
                        &row,
                        WebhookDeliveryJobCompletion::Discard {
                            now: now.to_owned(),
                        },
                    )?;
                    report.discarded += 1;
                    continue;
                }
            };

            let execution = match catch_unwind(AssertUnwindSafe(|| self.runtime.deliver(&job))) {
                Ok(Ok(execution)) => execution,
                Ok(Err(_)) | Err(_) => {
                    let completion = completion_for_runtime_error(now, &row)?;
                    self.complete_job(&row, completion)?;
                    if row.attempt >= row.max_attempts {
                        report.discarded += 1;
                    } else {
                        report.retried += 1;
                    }
                    continue;
                }
            };
            if matches!(
                &execution.outcome,
                DeliveryOutcome::Suppressed { reason } if reason == "circuit_open"
            ) {
                report.circuit_open_suppressed += 1;
            }
            match completion_for_execution(now, &execution)? {
                DrainCompletion::Retry { scheduled_at } => {
                    self.complete_job(&row, WebhookDeliveryJobCompletion::Retry { scheduled_at })?;
                    report.retried += 1;
                }
                DrainCompletion::Complete => {
                    self.complete_job(
                        &row,
                        WebhookDeliveryJobCompletion::Complete {
                            now: now.to_owned(),
                        },
                    )?;
                    report.completed += 1;
                }
                DrainCompletion::Discard => {
                    self.complete_job(
                        &row,
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
        job: &WebhookDeliveryJobRow,
        completion: WebhookDeliveryJobCompletion,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        let applied = store
            .complete_webhook_delivery_job(job, completion)
            .map_err(|error| error.to_string())?;
        if applied {
            Ok(())
        } else {
            Err(format!("webhook job {} execution lease lost", job.id))
        }
    }
}

fn completion_for_runtime_error(
    now: &str,
    row: &WebhookDeliveryJobRow,
) -> Result<WebhookDeliveryJobCompletion, String> {
    let max_attempts = if row.max_attempts == 0 {
        MAX_ATTEMPTS
    } else {
        row.max_attempts
    };
    if row.attempt >= max_attempts {
        return Ok(WebhookDeliveryJobCompletion::Discard {
            now: now.to_owned(),
        });
    }

    Ok(WebhookDeliveryJobCompletion::Retry {
        scheduled_at: add_seconds(now, 1)?,
    })
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

fn run_drain_worker(
    drain: WebhookDeliveryDrain,
    interval: StdDuration,
    stop: Arc<DrainStop>,
    health: WorkerHealthHandle,
) {
    while !stop.is_requested() {
        let now = current_rfc3339();
        match catch_unwind(AssertUnwindSafe(|| drain.drain_due(&now))) {
            Ok(Ok(report)) => health.record_success_with_pressure(
                now,
                current_unix_millis(),
                WorkerPressureSnapshot {
                    due_count: u64::from(report.due_count),
                    in_flight_count: u64::from(report.in_flight_count),
                    oldest_due_age_ms: report.oldest_due_age_ms,
                    backoff_or_circuit_open: report.retried > 0
                        || report.discarded > 0
                        || report.circuit_open_suppressed > 0
                        || report.recovery_retried > 0
                        || report.recovery_discarded > 0,
                },
            ),
            Ok(Err(_)) => health.record_failure("runtime_error"),
            Err(_) => health.record_failure("panic"),
        }
        if stop.wait(interval) {
            break;
        }
    }
    health.mark_stopped();
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
        attempt_timestamp: None,
    })
}

fn required_string(value: Option<&Value>, field: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("webhook job args missing {field}"))
}

fn add_seconds(now: &str, seconds: u64) -> Result<String, String> {
    let now = parse_drain_timestamp(now)?;
    now.checked_add(Duration::seconds(seconds as i64))
        .ok_or_else(|| "retry timestamp overflow".to_owned())?
        .format(&Rfc3339)
        .map_err(|error| format!("failed to format retry timestamp: {error}"))
}

fn subtract_seconds(now: &str, seconds: u64) -> Result<String, String> {
    let now = parse_drain_timestamp(now)?;
    now.checked_sub(Duration::seconds(seconds as i64))
        .ok_or_else(|| "lease recovery timestamp overflow".to_owned())?
        .format(&Rfc3339)
        .map_err(|error| format!("failed to format lease recovery timestamp: {error}"))
}

fn validate_drain_timestamp(now: &str) -> Result<(), String> {
    parse_drain_timestamp(now).map(|_| ())
}

fn oldest_due_age_ms(now: &str, oldest_due_at: Option<&str>) -> Result<Option<u64>, String> {
    let Some(oldest_due_at) = oldest_due_at else {
        return Ok(None);
    };
    let now = parse_drain_timestamp(now)?;
    let oldest_due_at = parse_drain_timestamp(oldest_due_at)?;
    if oldest_due_at >= now {
        return Ok(Some(0));
    }
    Ok(Some((now - oldest_due_at).whole_milliseconds() as u64))
}

fn parse_drain_timestamp(now: &str) -> Result<OffsetDateTime, String> {
    let now = OffsetDateTime::parse(now, &Rfc3339)
        .map_err(|error| format!("invalid drain timestamp: {error}"))?;
    Ok(now)
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration as StdDuration};

    use canary_workers::webhooks::{TransportResult, WebhookRequest};

    use super::*;

    struct NoopTransport;

    impl WebhookTransport for NoopTransport {
        fn send(&self, _request: &WebhookRequest) -> TransportResult {
            TransportResult::HttpStatus(204)
        }
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

    #[test]
    fn worker_records_lifecycle_failures() -> Result<(), Box<dyn std::error::Error>> {
        let store = Arc::new(Mutex::new(Store::open_in_memory()?));
        let runtime =
            WebhookDeliveryRuntime::new_without_circuit(store.clone(), Arc::new(NoopTransport));
        let drain = WebhookDeliveryDrain::new(store, runtime, 1);
        let worker = WebhookDeliveryDrainWorker::spawn(drain, StdDuration::from_millis(10))?;

        let deadline = std::time::Instant::now() + StdDuration::from_secs(1);
        while worker.failure_count() == 0 {
            if std::time::Instant::now() >= deadline {
                return Err("timed out waiting for webhook failure count".into());
            }
            thread::sleep(StdDuration::from_millis(10));
        }
        let snapshot = worker.health_snapshot();
        assert_eq!(snapshot.name, "webhook_delivery");
        assert!(snapshot.failure_count >= 1);
        assert_eq!(snapshot.last_error_class.as_deref(), Some("runtime_error"));

        worker.join()?;

        Ok(())
    }
}
