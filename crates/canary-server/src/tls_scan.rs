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
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{EventSink, current_rfc3339};

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
        let candidates = self.load_candidates()?;
        let mut report = TlsExpiryScanLifecycleReport {
            loaded: candidates.len(),
            ..TlsExpiryScanLifecycleReport::default()
        };

        for candidate in candidates {
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
    handle: Option<JoinHandle<()>>,
}

impl TlsExpiryScanLifecycleWorker {
    /// Spawn one named background thread that scans persisted TLS expiries.
    pub fn spawn(
        lifecycle: TlsExpiryScanLifecycle,
        config: TlsExpiryScanLifecycleConfig,
    ) -> Result<Self, String> {
        if config.tick_interval.is_zero() {
            return Err(
                "tls expiry scan lifecycle tick interval must be greater than zero".to_owned(),
            );
        }

        let control = Arc::new(LifecycleControl::default());
        let thread_control = control.clone();
        let handle = thread::Builder::new()
            .name("canary-tls-expiry-scan".to_owned())
            .spawn(move || run_lifecycle_worker(lifecycle, config.tick_interval, thread_control))
            .map_err(|error| format!("failed to spawn tls expiry scan worker: {error}"))?;

        Ok(Self {
            control,
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
        self.control.failure_count()
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
) {
    while !control.is_stopping() {
        if !control.is_paused() {
            match catch_unwind(AssertUnwindSafe(|| {
                let now = OffsetDateTime::now_utc();
                let now_string = now.format(&Rfc3339).unwrap_or_else(|_| current_rfc3339());
                lifecycle.run_due(now, now_string)
            })) {
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => control.record_failure(),
            }
        }
        if control.wait(interval) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Mutex as StdMutex;

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
        seed_tls_target(&mut store, "2026-06-05T00:00:00Z")?;
        let store = Arc::new(Mutex::new(store));
        let sink = Arc::new(RecordingSink::default());
        let lifecycle = TlsExpiryScanLifecycle::new(store.clone(), sink.clone());
        let now = OffsetDateTime::parse("2026-05-29T00:00:00Z", &Rfc3339)?;

        let report = lifecycle.run_due(now, "2026-05-29T00:00:00Z".to_owned())?;

        assert_eq!(
            report,
            TlsExpiryScanLifecycleReport {
                loaded: 1,
                planned: 1,
                recorded: 1,
                failed: 0,
                event_fanout_failed: 0,
            }
        );
        assert_eq!(
            sink.events.lock().map_err(|_| "events lock poisoned")?[0],
            "health_check.tls_expiring"
        );
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let timeline = store.timeline("24h", TimelineQueryOptions::default())?;
        let event = timeline.events.first().ok_or("missing timeline event")?;
        assert_eq!(event.event, "health_check.tls_expiring");
        assert_eq!(event.severity.as_deref(), Some("warning"));
        assert_eq!(event.summary, "api: TLS expires in 7 day(s)");
        assert_eq!(event.payload["tls_expires_at"], "2026-06-05T00:00:00Z");
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
        seed_tls_target(&mut store, "2026-06-05T00:00:00Z")?;
        let store = Arc::new(Mutex::new(store));
        let lifecycle = TlsExpiryScanLifecycle::new(store.clone(), Arc::new(FailingSink));
        let now = OffsetDateTime::parse("2026-05-29T00:00:00Z", &Rfc3339)?;

        let report = lifecycle.run_due(now, "2026-05-29T00:00:00Z".to_owned())?;

        assert_eq!(report.recorded, 1);
        assert_eq!(report.event_fanout_failed, 1);
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        let timeline = store.timeline("24h", TimelineQueryOptions::default())?;
        assert_eq!(timeline.events.len(), 1);
        assert_eq!(timeline.events[0].event, "health_check.tls_expiring");
        Ok(())
    }

    fn seed_tls_target(store: &mut Store, tls_expires_at: &str) -> Result<(), Box<dyn Error>> {
        store.insert_target(TargetInsert {
            id: "TGT-api".to_owned(),
            url: "https://api.example.test/healthz".to_owned(),
            name: "api-web".to_owned(),
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
            target_id: "TGT-api".to_owned(),
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
