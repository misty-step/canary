//! TLS-expiry scan runtime adapter.
//!
//! Target probes own network access and persist `target_checks.tls_expires_at`.
//! This worker only reads that persisted metadata, records the Phoenix-compatible
//! timeline warning, and fans out the committed event to webhooks.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration as StdDuration,
};

use canary_core::ids::EventId;
use canary_store::{Store, TlsExpiryEventInsert, TlsExpiryScanCandidate};
use canary_workers::tls_scan::{TlsExpiryScanInput, plan_tls_expiry_event};
use time::OffsetDateTime;

use crate::{
    EventSink, WorkerHealthHandle, WorkerName, WorkerPressureSnapshot,
    server_time::{current_unix_millis, current_utc, format_rfc3339},
};

/// Configuration for the TLS-expiry scan lifecycle worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsExpiryScanLifecycleConfig {
    /// Minimum delay between lifecycle passes.
    pub tick_interval: StdDuration,
}

impl Default for TlsExpiryScanLifecycleConfig {
    fn default() -> Self {
        Self {
            tick_interval: StdDuration::from_secs(24 * 60 * 60),
        }
    }
}

/// Summary of one TLS-expiry scan pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsExpiryScanLifecycleReport {
    /// Candidate rows loaded from the store.
    pub loaded: usize,
    /// Candidates that planned a warning event.
    pub planned: usize,
    /// Service events recorded in the store.
    pub recorded: usize,
    /// Candidates that failed before or during persistence.
    pub failed: usize,
    /// Advisory webhook enqueue failures after service-event commit.
    pub event_fanout_failed: usize,
    /// True when shutdown interrupted the pass between candidates.
    pub interrupted: bool,
}

/// Bounded lifecycle adapter for TLS-expiry warning evaluation.
pub struct TlsExpiryScanLifecycle {
    store: Arc<Mutex<Store>>,
    event_sink: Arc<dyn EventSink>,
}

impl TlsExpiryScanLifecycle {
    /// Build a lifecycle adapter from explicit persistence and fanout boundaries.
    pub fn new(store: Arc<Mutex<Store>>, event_sink: Arc<dyn EventSink>) -> Self {
        Self { store, event_sink }
    }

    /// Load candidate rows and evaluate them sequentially with one pass timestamp.
    pub fn run_due(
        &self,
        now: OffsetDateTime,
        now_string: String,
    ) -> Result<TlsExpiryScanLifecycleReport, String> {
        self.run_due_until(now, now_string, || false)
    }

    fn run_due_until(
        &self,
        now: OffsetDateTime,
        now_string: String,
        should_stop: impl Fn() -> bool,
    ) -> Result<TlsExpiryScanLifecycleReport, String> {
        let candidates = self.load_candidates()?;
        let mut report = TlsExpiryScanLifecycleReport {
            loaded: candidates.len(),
            ..TlsExpiryScanLifecycleReport::default()
        };

        for candidate in candidates {
            if should_stop() {
                report.interrupted = true;
                break;
            }
            match run_tls_expiry_scan_once(
                &self.store,
                self.event_sink.as_ref(),
                candidate,
                now,
                now_string.clone(),
            ) {
                Ok(true) => {
                    report.planned += 1;
                    report.recorded += 1;
                }
                Ok(false) => {}
                Err(TlsExpiryScanRuntimeError::EventFanout(_)) => {
                    report.planned += 1;
                    report.recorded += 1;
                    report.event_fanout_failed += 1;
                }
                Err(_) => report.failed += 1,
            }
        }

        Ok(report)
    }

    fn load_candidates(&self) -> Result<Vec<TlsExpiryScanCandidate>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        store
            .tls_expiry_scan_candidates()
            .map_err(|error| error.to_string())
    }
}

/// Dedicated OS-thread runner for TLS-expiry scan lifecycle passes.
pub struct TlsExpiryScanLifecycleWorker {
    control: Arc<LifecycleControl>,
    health: WorkerHealthHandle,
    handle: Option<JoinHandle<()>>,
}

impl TlsExpiryScanLifecycleWorker {
    /// Spawn one named background thread that scans persisted TLS expiries.
    pub fn spawn(
        lifecycle: TlsExpiryScanLifecycle,
        config: TlsExpiryScanLifecycleConfig,
    ) -> Result<Self, String> {
        Self::spawn_with_health(
            lifecycle,
            config,
            WorkerHealthHandle::new(WorkerName::TlsExpiryScan),
        )
    }

    /// Spawn one named background thread with an explicit health recorder.
    pub(crate) fn spawn_with_health(
        lifecycle: TlsExpiryScanLifecycle,
        config: TlsExpiryScanLifecycleConfig,
        health: WorkerHealthHandle,
    ) -> Result<Self, String> {
        if config.tick_interval.is_zero() {
            return Err(
                "tls expiry scan lifecycle tick interval must be greater than zero".to_owned(),
            );
        }

        let control = Arc::new(LifecycleControl::default());
        let thread_control = control.clone();
        health.mark_started();
        let thread_health = health.clone();
        let handle = thread::Builder::new()
            .name("canary-tls-expiry-scan".to_owned())
            .spawn(move || {
                run_lifecycle_worker(
                    lifecycle,
                    config.tick_interval,
                    thread_control,
                    thread_health,
                )
            })
            .map_err(|error| format!("failed to spawn tls expiry scan worker: {error}"))?;

        Ok(Self {
            control,
            health,
            handle: Some(handle),
        })
    }

    /// Pause future lifecycle passes without stopping the worker.
    pub fn pause(&self) {
        self.control.pause();
    }

    /// Resume lifecycle passes and wake the worker promptly.
    pub fn resume(&self) {
        self.control.resume();
    }

    /// Return lifecycle failures observed by this process.
    pub fn failure_count(&self) -> u64 {
        self.control
            .failure_count()
            .max(self.health.snapshot().failure_count)
    }

    /// Return the readiness-visible worker health snapshot.
    pub fn health_snapshot(&self) -> canary_http::public::WorkerReadyzCheck {
        self.health.snapshot()
    }

    /// Request shutdown without waiting for an in-flight pass to finish.
    pub fn stop(&self) {
        self.control.stop();
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
            Err(_) => Err("tls expiry scan worker panicked".to_owned()),
        }
    }
}

impl Drop for TlsExpiryScanLifecycleWorker {
    fn drop(&mut self) {
        self.stop();
        let _ = self.join_handle();
    }
}

/// Runtime failure for one TLS-expiry scan candidate.
#[derive(Debug, thiserror::Error)]
pub enum TlsExpiryScanRuntimeError {
    /// Store lock was poisoned.
    #[error("store lock poisoned")]
    StoreLock,
    /// Store returned an error.
    #[error("store error: {0}")]
    Store(#[from] canary_store::StoreError),
    /// Post-commit webhook enqueue failed.
    #[error("event fanout failed: {0}")]
    EventFanout(String),
}

/// Evaluate and persist exactly one TLS-expiry scan candidate.
pub fn run_tls_expiry_scan_once(
    store: &Arc<Mutex<Store>>,
    event_sink: &dyn EventSink,
    candidate: TlsExpiryScanCandidate,
    now: OffsetDateTime,
    now_string: String,
) -> Result<bool, TlsExpiryScanRuntimeError> {
    let Some(event) = plan_tls_expiry_event(scan_input(candidate), now) else {
        return Ok(false);
    };
    let commit = {
        let mut store = store
            .lock()
            .map_err(|_| TlsExpiryScanRuntimeError::StoreLock)?;
        store.record_tls_expiring_event(TlsExpiryEventInsert {
            event_id: EventId::generate(),
            target_id: event.target_id,
            name: event.name,
            service: event.service,
            url: event.url,
            tls_expires_at: event.tls_expires_at,
            days_until_expiry: event.days_until_expiry,
            now: now_string,
        })?
    };

    event_sink
        .enqueue_event(&commit.event, &commit.payload_json)
        .map_err(TlsExpiryScanRuntimeError::EventFanout)?;
    Ok(true)
}

fn scan_input(candidate: TlsExpiryScanCandidate) -> TlsExpiryScanInput {
    TlsExpiryScanInput {
        target_id: candidate.target_id,
        name: candidate.name,
        service: candidate.service,
        url: candidate.url,
        tls_expires_at: candidate.tls_expires_at,
    }
}

#[derive(Default)]
struct LifecycleControl {
    stopping: AtomicBool,
    paused: AtomicBool,
    failures: AtomicU64,
    lock: Mutex<()>,
    condvar: Condvar,
}

impl LifecycleControl {
    fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.condvar.notify_all();
    }

    fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        self.condvar.notify_all();
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.condvar.notify_all();
    }

    fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::SeqCst);
    }

    fn failure_count(&self) -> u64 {
        self.failures.load(Ordering::SeqCst)
    }

    fn wait(&self, interval: StdDuration) -> bool {
        if self.is_stopping() {
            return true;
        }

        let Ok(guard) = self.lock.lock() else {
            return true;
        };
        let _ = self
            .condvar
            .wait_timeout_while(guard, interval, |_| !self.stopping.load(Ordering::SeqCst));
        self.is_stopping()
    }
}

fn run_lifecycle_worker(
    lifecycle: TlsExpiryScanLifecycle,
    interval: StdDuration,
    control: Arc<LifecycleControl>,
    health: WorkerHealthHandle,
) {
    while !control.is_stopping() {
        if !control.is_paused() {
            let now = current_utc();
            let now_string = format_rfc3339(now);
            match catch_unwind(AssertUnwindSafe(|| {
                lifecycle.run_due_until(now, now_string.clone(), || control.is_stopping())
            })) {
                Ok(Ok(report)) => health.record_success_with_pressure(
                    now_string,
                    current_unix_millis(),
                    WorkerPressureSnapshot {
                        due_count: report.loaded as u64,
                        in_flight_count: 0,
                        oldest_due_age_ms: None,
                        backoff_or_circuit_open: report.failed > 0
                            || report.event_fanout_failed > 0
                            || report.interrupted,
                    },
                ),
                Ok(Err(_)) => {
                    control.record_failure();
                    health.record_failure("runtime_error");
                }
                Err(_) => {
                    control.record_failure();
                    health.record_failure("panic");
                }
            }
        }
        if control.wait(interval) {
            break;
        }
    }
    health.mark_stopped();
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use std::time::{Duration as StdDuration, Instant};

    use canary_store::{
        TargetCheckObservation, TargetInsert, TargetProbeCommit, TimelineQueryOptions,
    };
    use time::format_description::well_known::Rfc3339;

    use super::*;

    #[derive(Default)]
    struct RecordingSink {
        events: StdMutex<Vec<String>>,
    }

    impl EventSink for RecordingSink {
        fn enqueue_event(&self, event: &str, _payload_json: &str) -> Result<(), String> {
            self.events
                .lock()
                .map_err(|_| "events lock poisoned".to_owned())?
                .push(event.to_owned());
            Ok(())
        }
    }

    struct FailingSink;

    impl EventSink for FailingSink {
        fn enqueue_event(&self, event: &str, _payload_json: &str) -> Result<(), String> {
            Err(format!("simulated enqueue failure for {event}"))
        }
    }

    #[test]
    fn lifecycle_records_tls_expiring_event_and_enqueues_webhook() -> Result<(), Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        let now = current_utc();
        let now_string = format_rfc3339(now);
        let tls_expires_at = format_rfc3339(now + time::Duration::days(7));
        seed_tls_target(&mut store, &tls_expires_at)?;
        let store = Arc::new(Mutex::new(store));
        let sink = Arc::new(RecordingSink::default());
        let lifecycle = TlsExpiryScanLifecycle::new(store.clone(), sink.clone());

        let report = lifecycle.run_due(now, now_string)?;

        assert_eq!(
            report,
            TlsExpiryScanLifecycleReport {
                loaded: 1,
                planned: 1,
                recorded: 1,
                failed: 0,
                event_fanout_failed: 0,
                interrupted: false,
            }
        );
        assert_eq!(
            sink.events.lock().map_err(|_| "events lock poisoned")?[0],
            "health_check.tls_expiring"
        );
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let timeline = store.timeline("30d", TimelineQueryOptions::default())?;
        let event = timeline.events.first().ok_or("missing timeline event")?;
        assert_eq!(event.event, "health_check.tls_expiring");
        assert_eq!(event.severity.as_deref(), Some("warning"));
        assert_eq!(event.summary, "api: TLS expires in 7 day(s)");
        assert_eq!(event.payload["tls_expires_at"], tls_expires_at);
        assert_eq!(event.payload["days_until_expiry"], 7);
        assert_eq!(
            event.payload["target"]["url"],
            "https://api.example.test/healthz"
        );
        Ok(())
    }

    #[test]
    fn lifecycle_keeps_recorded_event_when_webhook_enqueue_fails() -> Result<(), Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        let now = current_utc();
        let now_string = format_rfc3339(now);
        let tls_expires_at = format_rfc3339(now + time::Duration::days(7));
        seed_tls_target(&mut store, &tls_expires_at)?;
        let store = Arc::new(Mutex::new(store));
        let lifecycle = TlsExpiryScanLifecycle::new(store.clone(), Arc::new(FailingSink));

        let report = lifecycle.run_due(now, now_string)?;

        assert_eq!(report.recorded, 1);
        assert_eq!(report.event_fanout_failed, 1);
        assert!(!report.interrupted);
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let timeline = store.timeline("30d", TimelineQueryOptions::default())?;
        assert_eq!(timeline.events.len(), 1);
        assert_eq!(timeline.events[0].event, "health_check.tls_expiring");
        Ok(())
    }

    #[test]
    fn worker_records_lifecycle_failures() -> Result<(), Box<dyn Error>> {
        let store = Arc::new(Mutex::new(Store::open_in_memory()?));
        let worker = TlsExpiryScanLifecycleWorker::spawn(
            TlsExpiryScanLifecycle::new(store, Arc::new(RecordingSink::default())),
            TlsExpiryScanLifecycleConfig {
                tick_interval: StdDuration::from_millis(10),
            },
        )?;

        let deadline = Instant::now() + StdDuration::from_secs(1);
        while worker.failure_count() == 0 {
            if Instant::now() >= deadline {
                return Err("timed out waiting for tls scan failure count".into());
            }
            thread::sleep(StdDuration::from_millis(10));
        }
        let snapshot = worker.health_snapshot();
        assert_eq!(snapshot.name, "tls_scan");
        assert!(snapshot.failure_count >= 1);
        assert_eq!(snapshot.last_error_class.as_deref(), Some("runtime_error"));

        worker.join()?;

        Ok(())
    }

    #[test]
    fn lifecycle_stops_between_candidates_when_shutdown_is_requested() -> Result<(), Box<dyn Error>>
    {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        seed_tls_target_with_id(&mut store, "TGT-api-a", "api-a", "2026-06-05T00:00:00Z")?;
        seed_tls_target_with_id(&mut store, "TGT-api-b", "api-b", "2026-06-05T00:00:00Z")?;
        let store = Arc::new(Mutex::new(store));
        let sink = Arc::new(RecordingSink::default());
        let lifecycle = TlsExpiryScanLifecycle::new(store, sink);
        let now = OffsetDateTime::parse("2026-05-29T00:00:00Z", &Rfc3339)?;
        let checks = AtomicU64::new(0);

        let report = lifecycle.run_due_until(now, "2026-05-29T00:00:00Z".to_owned(), || {
            checks.fetch_add(1, Ordering::SeqCst) > 0
        })?;

        assert_eq!(
            report,
            TlsExpiryScanLifecycleReport {
                loaded: 2,
                planned: 1,
                recorded: 1,
                failed: 0,
                event_fanout_failed: 0,
                interrupted: true,
            }
        );

        Ok(())
    }

    fn seed_tls_target(store: &mut Store, tls_expires_at: &str) -> Result<(), Box<dyn Error>> {
        seed_tls_target_with_id(store, "TGT-api", "api-web", tls_expires_at)
    }

    fn seed_tls_target_with_id(
        store: &mut Store,
        id: &str,
        name: &str,
        tls_expires_at: &str,
    ) -> Result<(), Box<dyn Error>> {
        store.insert_target(TargetInsert {
            id: id.to_owned(),
            url: "https://api.example.test/healthz".to_owned(),
            name: name.to_owned(),
            service: "api".to_owned(),
            method: "GET".to_owned(),
            headers: None,
            interval_ms: 60_000,
            timeout_ms: 10_000,
            expected_status: "200".to_owned(),
            body_contains: None,
            degraded_after: 1,
            down_after: 3,
            up_after: 1,
            active: true,
            created_at: "2026-05-29T00:00:00Z".to_owned(),
        })?;
        store.commit_target_probe(TargetProbeCommit {
            target_id: id.to_owned(),
            state: "up".to_owned(),
            consecutive_failures: 0,
            consecutive_successes: 1,
            check_succeeded: true,
            check: TargetCheckObservation {
                status_code: Some(200),
                latency_ms: Some(12),
                result: "success".to_owned(),
                tls_expires_at: Some(tls_expires_at.to_owned()),
                error_detail: None,
                region: None,
            },
            now: "2026-05-29T00:00:00Z".to_owned(),
            transition: None,
        })?;
        Ok(())
    }
}
