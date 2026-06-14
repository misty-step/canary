//! Non-HTTP monitor overdue runtime adapter.
//!
//! This module owns exactly one lifecycle pass: load monitor-state rows with
//! persisted deadlines, ask the pure worker planner what to do, commit through
//! the store command, and fan out already-recorded transition events.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration as StdDuration,
};

use canary_core::health::state_machine::HealthState;
use canary_store::{MonitorOverdueCandidate, Store};
use canary_workers::health::{
    HealthPlanError, MonitorMode, MonitorOverdueSnapshot, ObservationContext, plan_monitor_overdue,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    EventFanoutReport, HealthEventFanout, HealthEventSource, WorkerHealthHandle, WorkerName,
    WorkerPressureSnapshot,
    server_time::{current_rfc3339, current_unix_millis},
};

/// Persisted result of one overdue monitor transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorOverdueOutcome {
    /// Monitor id.
    pub monitor_id: String,
    /// Persisted monitor state after the overdue transition.
    pub state: String,
    /// Persisted monitor-state sequence after the transition.
    pub sequence: i64,
    /// Health transition event enqueued after commit.
    pub transition_event: String,
    /// Advisory webhook fanout result for the transition event.
    pub event_fanout: EventFanoutReport,
}

/// Runtime failure that prevented one overdue candidate from completing.
#[derive(Debug, thiserror::Error)]
pub enum MonitorOverdueRuntimeError {
    /// Store lock was poisoned.
    #[error("store lock poisoned")]
    StoreLock,
    /// Store returned an error.
    #[error("store error: {0}")]
    Store(#[from] canary_store::StoreError),
    /// Candidate row has unsupported persisted data.
    #[error("invalid monitor configuration: {0}")]
    InvalidMonitor(String),
    /// Planner rejected request-local data.
    #[error("monitor overdue planning failed: {0}")]
    Plan(#[from] HealthPlanError),
}

/// Runtime boundary for evaluating non-HTTP monitor overdue rows.
pub struct MonitorOverdueRuntime {
    store: Arc<Mutex<Store>>,
    health_fanout: HealthEventFanout,
}

impl MonitorOverdueRuntime {
    /// Build a monitor overdue runtime from explicit side-effect boundaries.
    pub fn new(store: Arc<Mutex<Store>>, health_fanout: HealthEventFanout) -> Self {
        Self {
            store,
            health_fanout,
        }
    }

    /// Evaluate and persist exactly one overdue candidate.
    pub fn run_candidate(
        &self,
        candidate: MonitorOverdueCandidate,
        now: String,
        now_millis: i64,
    ) -> Result<Option<MonitorOverdueOutcome>, MonitorOverdueRuntimeError> {
        run_monitor_overdue_once(&self.store, &self.health_fanout, candidate, now, now_millis)
    }
}

/// Configuration for the monitor overdue lifecycle worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorOverdueLifecycleConfig {
    /// Minimum delay between lifecycle passes.
    pub tick_interval: StdDuration,
}

impl Default for MonitorOverdueLifecycleConfig {
    fn default() -> Self {
        Self {
            tick_interval: StdDuration::from_secs(1),
        }
    }
}

/// Summary of one lifecycle pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonitorOverdueLifecycleReport {
    /// Candidate rows loaded from the store.
    pub loaded: usize,
    /// Age in milliseconds of the oldest loaded overdue deadline.
    pub oldest_due_age_ms: Option<u64>,
    /// Candidates that produced no transition.
    pub noop: usize,
    /// Candidates that committed an overdue transition.
    pub transitioned: usize,
    /// Candidates that failed before or during commit.
    pub failed: usize,
    /// Advisory health-transition webhook enqueue failures.
    pub event_fanout_failed: usize,
    /// True when shutdown interrupted the pass between candidates.
    pub interrupted: bool,
}

/// Bounded lifecycle adapter for monitor overdue evaluation.
pub struct MonitorOverdueLifecycle {
    store: Arc<Mutex<Store>>,
    runtime: MonitorOverdueRuntime,
}

impl MonitorOverdueLifecycle {
    /// Build a lifecycle adapter from the shared store and overdue runtime.
    pub fn new(store: Arc<Mutex<Store>>, runtime: MonitorOverdueRuntime) -> Self {
        Self { store, runtime }
    }

    /// Load candidate rows and evaluate them sequentially with one pass timestamp.
    pub fn run_due(
        &self,
        now: String,
        now_millis: i64,
    ) -> Result<MonitorOverdueLifecycleReport, String> {
        self.run_due_until(now, now_millis, || false)
    }

    fn run_due_until(
        &self,
        now: String,
        now_millis: i64,
        should_stop: impl Fn() -> bool,
    ) -> Result<MonitorOverdueLifecycleReport, String> {
        let candidates = self.load_candidates()?;
        let mut report = MonitorOverdueLifecycleReport {
            loaded: candidates.len(),
            oldest_due_age_ms: oldest_monitor_due_age_ms(now_millis, &candidates),
            ..MonitorOverdueLifecycleReport::default()
        };

        for candidate in candidates {
            if should_stop() {
                report.interrupted = true;
                break;
            }
            match self
                .runtime
                .run_candidate(candidate, now.clone(), now_millis)
            {
                Ok(Some(outcome)) => {
                    report.transitioned += 1;
                    report.event_fanout_failed += outcome.event_fanout.failed;
                }
                Ok(None) => report.noop += 1,
                Err(_) => report.failed += 1,
            }
        }

        Ok(report)
    }

    fn load_candidates(&self) -> Result<Vec<MonitorOverdueCandidate>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        store
            .monitor_overdue_candidates()
            .map_err(|error| error.to_string())
    }
}

/// Dedicated OS-thread runner for monitor overdue lifecycle passes.
pub struct MonitorOverdueLifecycleWorker {
    control: Arc<LifecycleControl>,
    health: WorkerHealthHandle,
    handle: Option<JoinHandle<()>>,
}

impl MonitorOverdueLifecycleWorker {
    /// Spawn one named background thread that evaluates overdue monitors sequentially.
    pub fn spawn(
        lifecycle: MonitorOverdueLifecycle,
        config: MonitorOverdueLifecycleConfig,
    ) -> Result<Self, String> {
        Self::spawn_with_health(
            lifecycle,
            config,
            WorkerHealthHandle::new(WorkerName::MonitorOverdue),
        )
    }

    /// Spawn one named background thread with an explicit health recorder.
    pub(crate) fn spawn_with_health(
        lifecycle: MonitorOverdueLifecycle,
        config: MonitorOverdueLifecycleConfig,
        health: WorkerHealthHandle,
    ) -> Result<Self, String> {
        if config.tick_interval.is_zero() {
            return Err(
                "monitor overdue lifecycle tick interval must be greater than zero".to_owned(),
            );
        }

        let control = Arc::new(LifecycleControl::default());
        let thread_control = control.clone();
        health.mark_started();
        let thread_health = health.clone();
        let handle = thread::Builder::new()
            .name("canary-monitor-overdue".to_owned())
            .spawn(move || {
                run_lifecycle_worker(
                    lifecycle,
                    config.tick_interval,
                    thread_control,
                    thread_health,
                )
            })
            .map_err(|error| format!("failed to spawn monitor overdue worker: {error}"))?;

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
            Err(_) => Err("monitor overdue worker panicked".to_owned()),
        }
    }
}

impl Drop for MonitorOverdueLifecycleWorker {
    fn drop(&mut self) {
        self.stop();
        let _ = self.join_handle();
    }
}

/// Evaluate and persist exactly one overdue monitor candidate.
pub fn run_monitor_overdue_once(
    store: &Arc<Mutex<Store>>,
    health_fanout: &HealthEventFanout,
    candidate: MonitorOverdueCandidate,
    now: String,
    now_millis: i64,
) -> Result<Option<MonitorOverdueOutcome>, MonitorOverdueRuntimeError> {
    let Some(snapshot) = monitor_overdue_snapshot(candidate) else {
        return Ok(None);
    };
    let context = ObservationContext {
        now,
        now_millis,
        event_id: canary_core::ids::EventId::generate(),
        incident_id: canary_core::ids::IncidentId::generate(),
        incident_event_id: canary_core::ids::EventId::generate(),
    };
    let Some(plan) = plan_monitor_overdue(snapshot, context)? else {
        return Ok(None);
    };
    let response_monitor_id = plan.commit.monitor_id.clone();
    let response_state = plan.commit.state.clone();
    let commit = {
        let mut store = store
            .lock()
            .map_err(|_| MonitorOverdueRuntimeError::StoreLock)?;
        store.commit_monitor_overdue(plan.commit)?
    };

    let event_fanout =
        health_fanout.dispatch(HealthEventSource::MonitorOverdue, &commit.transition);

    Ok(Some(MonitorOverdueOutcome {
        monitor_id: response_monitor_id,
        state: response_state,
        sequence: commit.sequence,
        transition_event: commit.transition.event,
        event_fanout,
    }))
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
    lifecycle: MonitorOverdueLifecycle,
    interval: StdDuration,
    control: Arc<LifecycleControl>,
    health: WorkerHealthHandle,
) {
    while !control.is_stopping() {
        if !control.is_paused() {
            let now = current_rfc3339();
            match catch_unwind(AssertUnwindSafe(|| {
                lifecycle.run_due(now.clone(), current_unix_millis())
            })) {
                Ok(Ok(report)) => health.record_success_with_pressure(
                    now,
                    current_unix_millis(),
                    WorkerPressureSnapshot {
                        due_count: report.loaded as u64,
                        in_flight_count: 0,
                        oldest_due_age_ms: report.oldest_due_age_ms,
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

fn monitor_overdue_snapshot(candidate: MonitorOverdueCandidate) -> Option<MonitorOverdueSnapshot> {
    Some(MonitorOverdueSnapshot {
        id: candidate.id,
        name: candidate.name,
        service: candidate.service,
        mode: monitor_mode(&candidate.mode)?,
        expected_every_ms: candidate.expected_every_ms,
        grace_ms: candidate.grace_ms,
        state: health_state(&candidate.state)?,
        last_check_in_status: candidate.last_check_in_status,
        last_check_in_at: candidate.last_check_in_at,
        deadline_at: candidate.deadline_at,
        first_missed_at: candidate.first_missed_at,
    })
}

fn oldest_monitor_due_age_ms(
    now_millis: i64,
    candidates: &[MonitorOverdueCandidate],
) -> Option<u64> {
    candidates
        .iter()
        .filter_map(|candidate| candidate.deadline_at.as_deref())
        .filter_map(|deadline| OffsetDateTime::parse(deadline, &Rfc3339).ok())
        .map(|deadline| {
            let deadline_millis = deadline
                .unix_timestamp()
                .saturating_mul(1_000)
                .saturating_add(i64::from(deadline.millisecond()));
            now_millis.saturating_sub(deadline_millis)
        })
        .map(|age| age.max(0) as u64)
        .max()
}

fn monitor_mode(value: &str) -> Option<MonitorMode> {
    match value {
        "schedule" => Some(MonitorMode::Schedule),
        "ttl" => Some(MonitorMode::Ttl),
        _ => None,
    }
}

fn health_state(value: &str) -> Option<HealthState> {
    HealthState::parse_persisted(value)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use std::time::Instant;

    use canary_store::{
        MonitorCheckInCommit, MonitorCheckInObservation, MonitorInsert, MonitorOverdueCandidate,
    };

    use crate::EventSink;

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
    fn lifecycle_commits_degraded_then_down_without_synthetic_check_ins()
    -> Result<(), Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        store.insert_monitor(MonitorInsert {
            id: "MON-overdue".to_owned(),
            name: "Overdue worker".to_owned(),
            service: "worker".to_owned(),
            mode: "schedule".to_owned(),
            expected_every_ms: 60_000,
            grace_ms: 5_000,
            created_at: "2026-05-28T19:00:00Z".to_owned(),
        })?;
        store.commit_monitor_check_in(MonitorCheckInCommit {
            monitor_id: "MON-overdue".to_owned(),
            state: "up".to_owned(),
            last_check_in_at: Some("2026-05-28T20:00:00Z".to_owned()),
            last_check_in_status: Some("alive".to_owned()),
            deadline_at: Some("2026-05-28T20:00:05Z".to_owned()),
            check_in: MonitorCheckInObservation {
                id: "CHK-overduealive".to_owned(),
                external_id: None,
                status: "alive".to_owned(),
                observed_at: "2026-05-28T20:00:00Z".to_owned(),
                ttl_ms: None,
                summary: None,
                context: None,
            },
            now: "2026-05-28T20:00:00Z".to_owned(),
            transition: None,
        })?;

        let store = Arc::new(Mutex::new(store));
        let sink = Arc::new(RecordingSink::default());
        let fanout = HealthEventFanout::new_without_failure_sink(sink.clone());
        let lifecycle =
            MonitorOverdueLifecycle::new(store.clone(), MonitorOverdueRuntime::new(store, fanout));

        let degraded = lifecycle.run_due("2026-05-28T20:00:06Z".to_owned(), 0)?;
        assert_eq!(
            degraded,
            MonitorOverdueLifecycleReport {
                loaded: 1,
                oldest_due_age_ms: Some(0),
                noop: 0,
                transitioned: 1,
                failed: 0,
                event_fanout_failed: 0,
                interrupted: false,
            }
        );
        let waiting = lifecycle.run_due("2026-05-28T20:00:30Z".to_owned(), 0)?;
        assert_eq!(
            waiting,
            MonitorOverdueLifecycleReport {
                loaded: 1,
                oldest_due_age_ms: Some(0),
                noop: 1,
                transitioned: 0,
                failed: 0,
                event_fanout_failed: 0,
                interrupted: false,
            }
        );
        let down = lifecycle.run_due("2026-05-28T20:01:06Z".to_owned(), 0)?;
        assert_eq!(down.transitioned, 1);

        let events = sink
            .events
            .lock()
            .map_err(|_| "events lock poisoned")?
            .clone();
        assert!(events.contains(&"health_check.degraded".to_owned()));
        assert!(events.contains(&"health_check.down".to_owned()));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.as_str() == "incident.opened")
                .count(),
            1
        );

        Ok(())
    }

    #[test]
    fn lifecycle_reports_enqueue_failures_without_failing_transition() -> Result<(), Box<dyn Error>>
    {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        store.insert_monitor(MonitorInsert {
            id: "MON-overdue".to_owned(),
            name: "Overdue worker".to_owned(),
            service: "worker".to_owned(),
            mode: "schedule".to_owned(),
            expected_every_ms: 60_000,
            grace_ms: 5_000,
            created_at: "2026-05-28T19:00:00Z".to_owned(),
        })?;
        store.commit_monitor_check_in(MonitorCheckInCommit {
            monitor_id: "MON-overdue".to_owned(),
            state: "up".to_owned(),
            last_check_in_at: Some("2026-05-28T20:00:00Z".to_owned()),
            last_check_in_status: Some("alive".to_owned()),
            deadline_at: Some("2026-05-28T20:00:05Z".to_owned()),
            check_in: MonitorCheckInObservation {
                id: "CHK-overduealive".to_owned(),
                external_id: None,
                status: "alive".to_owned(),
                observed_at: "2026-05-28T20:00:00Z".to_owned(),
                ttl_ms: None,
                summary: None,
                context: None,
            },
            now: "2026-05-28T20:00:00Z".to_owned(),
            transition: None,
        })?;

        let store = Arc::new(Mutex::new(store));
        let recorder = Arc::new(crate::EnqueueFailureRecorder::default());
        let lifecycle = MonitorOverdueLifecycle::new(
            store.clone(),
            MonitorOverdueRuntime::new(
                store,
                HealthEventFanout::new(Arc::new(FailingSink), recorder.clone()),
            ),
        );

        let report = lifecycle.run_due("2026-05-28T20:00:06Z".to_owned(), 0)?;

        assert_eq!(report.transitioned, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(report.event_fanout_failed, 2);
        assert!(!report.interrupted);
        assert_eq!(
            recorder.snapshot().get(&crate::EnqueueFailureKey {
                source: HealthEventSource::MonitorOverdue,
                event: "health_check.degraded".to_owned(),
            }),
            Some(&1)
        );
        Ok(())
    }

    #[test]
    fn lifecycle_stops_between_candidates_when_shutdown_is_requested() -> Result<(), Box<dyn Error>>
    {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        seed_overdue_monitor(&mut store, "MON-overdue-a")?;
        seed_overdue_monitor(&mut store, "MON-overdue-b")?;

        let store = Arc::new(Mutex::new(store));
        let sink = Arc::new(RecordingSink::default());
        let fanout = HealthEventFanout::new_without_failure_sink(sink);
        let lifecycle =
            MonitorOverdueLifecycle::new(store.clone(), MonitorOverdueRuntime::new(store, fanout));
        let checks = AtomicU64::new(0);

        let report = lifecycle.run_due_until("2026-05-28T20:00:06Z".to_owned(), 0, || {
            checks.fetch_add(1, Ordering::SeqCst) > 0
        })?;

        assert_eq!(
            report,
            MonitorOverdueLifecycleReport {
                loaded: 2,
                oldest_due_age_ms: Some(0),
                noop: 0,
                transitioned: 1,
                failed: 0,
                event_fanout_failed: 0,
                interrupted: true,
            }
        );

        Ok(())
    }

    #[test]
    fn worker_records_lifecycle_failures() -> Result<(), Box<dyn Error>> {
        let store = Arc::new(Mutex::new(Store::open_in_memory()?));
        let sink = Arc::new(RecordingSink::default());
        let fanout = HealthEventFanout::new_without_failure_sink(sink);
        let worker = MonitorOverdueLifecycleWorker::spawn(
            MonitorOverdueLifecycle::new(store.clone(), MonitorOverdueRuntime::new(store, fanout)),
            MonitorOverdueLifecycleConfig {
                tick_interval: StdDuration::from_millis(10),
            },
        )?;

        let deadline = Instant::now() + StdDuration::from_secs(1);
        while worker.failure_count() == 0 {
            if Instant::now() >= deadline {
                return Err("timed out waiting for monitor overdue failure count".into());
            }
            thread::sleep(StdDuration::from_millis(10));
        }
        let snapshot = worker.health_snapshot();
        assert_eq!(snapshot.name, "monitor_overdue");
        assert!(snapshot.failure_count >= 1);
        assert_eq!(snapshot.last_error_class.as_deref(), Some("runtime_error"));

        worker.join()?;

        Ok(())
    }

    #[test]
    fn run_candidate_treats_unsupported_persisted_monitor_rows_as_noop()
    -> Result<(), Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        let store = Arc::new(Mutex::new(store));
        let sink = Arc::new(RecordingSink::default());
        let fanout = HealthEventFanout::new_without_failure_sink(sink.clone());

        let outcome = run_monitor_overdue_once(
            &store,
            &fanout,
            MonitorOverdueCandidate {
                id: "MON-unsupported".to_owned(),
                name: "Unsupported worker".to_owned(),
                service: "worker".to_owned(),
                mode: "unsupported".to_owned(),
                expected_every_ms: 60_000,
                grace_ms: 5_000,
                state: "up".to_owned(),
                last_check_in_status: Some("alive".to_owned()),
                last_check_in_at: Some("2026-05-28T19:59:00Z".to_owned()),
                deadline_at: Some("2026-05-28T20:00:05Z".to_owned()),
                first_missed_at: None,
            },
            "2026-05-28T20:00:06Z".to_owned(),
            0,
        )?;

        assert!(outcome.is_none());
        assert!(
            sink.events
                .lock()
                .map_err(|_| "events lock poisoned")?
                .is_empty()
        );
        Ok(())
    }

    fn seed_overdue_monitor(store: &mut Store, id: &str) -> Result<(), Box<dyn Error>> {
        store.insert_monitor(MonitorInsert {
            id: id.to_owned(),
            name: format!("Overdue worker {id}"),
            service: "worker".to_owned(),
            mode: "schedule".to_owned(),
            expected_every_ms: 60_000,
            grace_ms: 5_000,
            created_at: "2026-05-28T19:00:00Z".to_owned(),
        })?;
        store.commit_monitor_check_in(MonitorCheckInCommit {
            monitor_id: id.to_owned(),
            state: "up".to_owned(),
            last_check_in_at: Some("2026-05-28T20:00:00Z".to_owned()),
            last_check_in_status: Some("alive".to_owned()),
            deadline_at: Some("2026-05-28T20:00:05Z".to_owned()),
            check_in: MonitorCheckInObservation {
                id: format!("CHK-{id}-alive"),
                external_id: None,
                status: "alive".to_owned(),
                observed_at: "2026-05-28T20:00:00Z".to_owned(),
                ttl_ms: None,
                summary: None,
                context: None,
            },
            now: "2026-05-28T20:00:00Z".to_owned(),
            transition: None,
        })?;
        Ok(())
    }
}
