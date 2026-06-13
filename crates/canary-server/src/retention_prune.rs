//! Retention prune runtime adapter.
//!
//! The worker owns cadence and process lifecycle. `canary-workers` owns cutoff
//! planning and `canary-store` owns the fixed SQL table set. This module keeps
//! the shared store mutex scoped to one 1,000-row delete statement at a time.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration as StdDuration,
};

use canary_store::{RetentionPruneBatch, RetentionPruneTable, Store};
use canary_workers::retention::{RetentionPolicy, plan_retention_prune};
use time::OffsetDateTime;

use crate::{
    WorkerHealthHandle, WorkerName,
    server_time::{current_utc, format_rfc3339},
};

/// Configuration for the retention prune lifecycle worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPruneLifecycleConfig {
    /// Minimum delay between retention prune passes.
    pub tick_interval: StdDuration,
}

impl Default for RetentionPruneLifecycleConfig {
    fn default() -> Self {
        Self {
            tick_interval: StdDuration::from_secs(24 * 60 * 60),
        }
    }
}

/// Summary of one retention prune pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionPruneLifecycleReport {
    /// Deleted error rows.
    pub errors_deleted: u64,
    /// Deleted service-event rows.
    pub service_events_deleted: u64,
    /// Deleted target-check rows.
    pub target_checks_deleted: u64,
    /// Store delete statements executed.
    pub batches: u64,
    /// True when shutdown interrupted the pass after a completed batch.
    pub interrupted: bool,
}

/// Bounded lifecycle adapter for retention pruning.
pub struct RetentionPruneLifecycle {
    store: Arc<Mutex<Store>>,
    policy: RetentionPolicy,
}

impl RetentionPruneLifecycle {
    /// Build a lifecycle adapter from the shared store and retention policy.
    pub fn new(store: Arc<Mutex<Store>>, policy: RetentionPolicy) -> Self {
        Self { store, policy }
    }

    /// Execute one retention prune pass from one observed clock value.
    pub fn run_due(&self, now: OffsetDateTime) -> Result<RetentionPruneLifecycleReport, String> {
        self.run_due_until(now, || false)
    }

    fn run_due_until(
        &self,
        now: OffsetDateTime,
        should_stop: impl Fn() -> bool,
    ) -> Result<RetentionPruneLifecycleReport, String> {
        let plan = plan_retention_prune(self.policy, now)?;
        let mut report = RetentionPruneLifecycleReport::default();

        if should_stop() {
            report.interrupted = true;
            return Ok(report);
        }

        report.interrupted = self.prune_table(
            RetentionPruneTable::Errors,
            plan.error_cutoff.clone(),
            |deleted| report.errors_deleted += deleted,
            &mut report.batches,
            &should_stop,
        )?;
        if report.interrupted {
            return Ok(report);
        }
        report.interrupted = self.prune_table(
            RetentionPruneTable::ServiceEvents,
            plan.error_cutoff,
            |deleted| report.service_events_deleted += deleted,
            &mut report.batches,
            &should_stop,
        )?;
        if report.interrupted {
            return Ok(report);
        }
        report.interrupted = self.prune_table(
            RetentionPruneTable::TargetChecks,
            plan.check_cutoff,
            |deleted| report.target_checks_deleted += deleted,
            &mut report.batches,
            &should_stop,
        )?;

        Ok(report)
    }

    fn prune_table(
        &self,
        table: RetentionPruneTable,
        cutoff: String,
        mut add_deleted: impl FnMut(u64),
        batches: &mut u64,
        should_stop: &impl Fn() -> bool,
    ) -> Result<bool, String> {
        loop {
            let batch = {
                let mut store = self
                    .store
                    .lock()
                    .map_err(|_| "store lock poisoned".to_owned())?;
                store
                    .prune_retention_batch(RetentionPruneBatch {
                        table,
                        cutoff: cutoff.clone(),
                    })
                    .map_err(|error| error.to_string())?
            };
            *batches += 1;
            add_deleted(batch.deleted);
            if batch.complete {
                return Ok(false);
            }
            if should_stop() {
                return Ok(true);
            }
        }
    }
}

/// Dedicated OS-thread runner for retention prune lifecycle passes.
pub struct RetentionPruneLifecycleWorker {
    control: Arc<LifecycleControl>,
    health: WorkerHealthHandle,
    handle: Option<JoinHandle<()>>,
}

impl RetentionPruneLifecycleWorker {
    /// Spawn one named background thread that prunes old rows sequentially.
    pub fn spawn(
        lifecycle: RetentionPruneLifecycle,
        config: RetentionPruneLifecycleConfig,
    ) -> Result<Self, String> {
        Self::spawn_with_health(
            lifecycle,
            config,
            WorkerHealthHandle::new(WorkerName::RetentionPrune),
        )
    }

    /// Spawn one named background thread with an explicit health recorder.
    pub(crate) fn spawn_with_health(
        lifecycle: RetentionPruneLifecycle,
        config: RetentionPruneLifecycleConfig,
        health: WorkerHealthHandle,
    ) -> Result<Self, String> {
        if config.tick_interval.is_zero() {
            return Err(
                "retention prune lifecycle tick interval must be greater than zero".to_owned(),
            );
        }

        let control = Arc::new(LifecycleControl::default());
        let thread_control = control.clone();
        health.mark_started();
        let thread_health = health.clone();
        let handle = thread::Builder::new()
            .name("canary-retention-prune".to_owned())
            .spawn(move || {
                run_lifecycle_worker(
                    lifecycle,
                    config.tick_interval,
                    thread_control,
                    thread_health,
                )
            })
            .map_err(|error| format!("failed to spawn retention prune worker: {error}"))?;

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

    /// Request shutdown without waiting for an in-flight prune pass to finish.
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
            Err(_) => Err("retention prune worker panicked".to_owned()),
        }
    }
}

impl Drop for RetentionPruneLifecycleWorker {
    fn drop(&mut self) {
        self.stop();
        let _ = self.join_handle();
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
    lifecycle: RetentionPruneLifecycle,
    interval: StdDuration,
    control: Arc<LifecycleControl>,
    health: WorkerHealthHandle,
) {
    while !control.is_stopping() {
        if !control.is_paused() {
            let now = current_utc();
            match catch_unwind(AssertUnwindSafe(|| {
                lifecycle.run_due_until(now, || control.is_stopping())
            })) {
                Ok(Ok(_)) => health.record_success(format_rfc3339(now)),
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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration as StdDuration, Instant};

    use canary_core::{
        ids::{ErrorId, EventId},
        ingest::classification::{Category, Classification, Component, Persistence},
    };
    use canary_store::{ErrorIngest, ErrorIngestIds, ErrorIngestPayload};
    use time::format_description::well_known::Rfc3339;

    use super::*;

    #[test]
    fn lifecycle_prunes_all_tables_and_reports_batches() -> Result<(), Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        for index in 0..1005 {
            store.commit_error_ingest(error_ingest(index, "2026-04-01T00:00:00Z"))?;
        }

        let lifecycle = RetentionPruneLifecycle::new(
            Arc::new(Mutex::new(store)),
            RetentionPolicy {
                error_retention_days: 30,
                check_retention_days: 7,
            },
        );
        let report = lifecycle.run_due(OffsetDateTime::parse("2026-05-29T12:00:00Z", &Rfc3339)?)?;

        assert_eq!(
            report,
            RetentionPruneLifecycleReport {
                errors_deleted: 1005,
                service_events_deleted: 1005,
                target_checks_deleted: 0,
                batches: 5,
                interrupted: false,
            }
        );

        Ok(())
    }

    #[test]
    fn lifecycle_stops_after_current_batch_when_shutdown_is_requested() -> Result<(), Box<dyn Error>>
    {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        for index in 0..1005 {
            store.commit_error_ingest(error_ingest(index, "2026-04-01T00:00:00Z"))?;
        }

        let lifecycle =
            RetentionPruneLifecycle::new(Arc::new(Mutex::new(store)), RetentionPolicy::default());
        let checks = AtomicU64::new(0);
        let report = lifecycle.run_due_until(
            OffsetDateTime::parse("2026-05-29T12:00:00Z", &Rfc3339)?,
            || checks.fetch_add(1, Ordering::SeqCst) > 0,
        )?;

        assert_eq!(
            report,
            RetentionPruneLifecycleReport {
                errors_deleted: 1000,
                service_events_deleted: 0,
                target_checks_deleted: 0,
                batches: 1,
                interrupted: true,
            }
        );

        Ok(())
    }

    #[test]
    fn worker_records_planning_failures() -> Result<(), Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        let worker = RetentionPruneLifecycleWorker::spawn(
            RetentionPruneLifecycle::new(
                Arc::new(Mutex::new(store)),
                RetentionPolicy {
                    error_retention_days: -1,
                    check_retention_days: 7,
                },
            ),
            RetentionPruneLifecycleConfig {
                tick_interval: StdDuration::from_millis(10),
            },
        )?;

        let deadline = Instant::now() + StdDuration::from_secs(1);
        while worker.failure_count() == 0 {
            if Instant::now() >= deadline {
                return Err("timed out waiting for retention failure count".into());
            }
            thread::sleep(StdDuration::from_millis(10));
        }
        let snapshot = worker.health_snapshot();
        assert_eq!(snapshot.name, "retention_prune");
        assert!(snapshot.failure_count >= 1);
        assert_eq!(snapshot.last_error_class.as_deref(), Some("runtime_error"));

        worker.join()?;

        Ok(())
    }

    fn error_ingest(index: usize, created_at: &str) -> ErrorIngest {
        ErrorIngest {
            ids: ErrorIngestIds {
                error_id: ErrorId::generate(),
                event_id: EventId::generate(),
            },
            payload: ErrorIngestPayload {
                service: "retention".to_owned(),
                error_class: "RuntimeError".to_owned(),
                message: "old".to_owned(),
                message_template: "old".to_owned(),
                stack_trace: None,
                context_json: None,
                severity: "error".to_owned(),
                environment: "production".to_owned(),
                group_hash: format!("grp-old-{index}"),
                fingerprint_json: None,
                region: None,
                classification: Classification {
                    category: Category::Application,
                    persistence: Persistence::Persistent,
                    component: Component::Runtime,
                },
                created_at: created_at.to_owned(),
            },
        }
    }
}
