//! Process-local lifecycle health for background workers.
//!
//! Worker health is runtime state, not durable product data. This module keeps
//! the mutable counters in memory and exposes a narrow redacted snapshot for
//! readiness responses.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
};

use canary_http::public::{
    WorkerDueItem, WorkerHealthStatus, WorkerLifecycleState, WorkerPressureShape, WorkerReadyzCheck,
};

/// Stable names for lifecycle workers exposed through readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerName {
    /// Scheduled webhook delivery drain.
    WebhookDelivery,
    /// HTTP target probe scheduler.
    TargetProbe,
    /// Non-HTTP monitor overdue evaluator.
    MonitorOverdue,
    /// Retention pruning maintenance worker.
    RetentionPrune,
    /// TLS certificate expiry scanner.
    TlsExpiryScan,
}

impl WorkerName {
    /// Stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WebhookDelivery => "webhook_delivery",
            Self::TargetProbe => "target_probe",
            Self::MonitorOverdue => "monitor_overdue",
            Self::RetentionPrune => "retention_prune",
            Self::TlsExpiryScan => "tls_scan",
        }
    }

    const fn stale_after_ms(self) -> i64 {
        match self {
            Self::WebhookDelivery | Self::TargetProbe | Self::MonitorOverdue => 30_000,
            Self::RetentionPrune | Self::TlsExpiryScan => 25 * 60 * 60 * 1_000,
        }
    }

    const fn pressure_after_ms(self) -> u64 {
        match self {
            Self::WebhookDelivery | Self::TargetProbe | Self::MonitorOverdue => 120_000,
            Self::RetentionPrune | Self::TlsExpiryScan => 25 * 60 * 60 * 1_000,
        }
    }

    /// How this worker's `due_count`/`oldest_due_age_ms` fields should be read.
    ///
    /// Root-caused by canary-911: retention_prune and tls_scan sweep everything past
    /// a cutoff in one pass rather than draining a queue, so their `due_count` is a
    /// last-pass volume report, not a live backlog. Callers must branch on this shape
    /// before treating a large `due_count` or a sub-`stale_after_ms` cadence gap as
    /// pressure.
    pub const fn pressure_shape(self) -> WorkerPressureShape {
        match self {
            Self::WebhookDelivery | Self::TargetProbe | Self::MonitorOverdue => {
                WorkerPressureShape::Queue
            }
            Self::RetentionPrune | Self::TlsExpiryScan => WorkerPressureShape::SweepResult,
        }
    }
}

const LIFECYCLE_WORKERS: [WorkerName; 5] = [
    WorkerName::WebhookDelivery,
    WorkerName::TargetProbe,
    WorkerName::MonitorOverdue,
    WorkerName::RetentionPrune,
    WorkerName::TlsExpiryScan,
];

/// Snapshot source for all runtime lifecycle workers.
#[derive(Debug, Clone)]
pub struct WorkerHealthRegistry {
    webhook_delivery: WorkerHealthHandle,
    target_probe: WorkerHealthHandle,
    monitor_overdue: WorkerHealthHandle,
    retention_prune: WorkerHealthHandle,
    tls_expiry_scan: WorkerHealthHandle,
}

impl WorkerHealthRegistry {
    /// Build a registry with one handle for every Canary lifecycle worker.
    pub fn new() -> Self {
        Self {
            webhook_delivery: WorkerHealthHandle::new(WorkerName::WebhookDelivery),
            target_probe: WorkerHealthHandle::new(WorkerName::TargetProbe),
            monitor_overdue: WorkerHealthHandle::new(WorkerName::MonitorOverdue),
            retention_prune: WorkerHealthHandle::new(WorkerName::RetentionPrune),
            tls_expiry_scan: WorkerHealthHandle::new(WorkerName::TlsExpiryScan),
        }
    }

    /// Return the webhook-delivery worker handle.
    pub fn webhook_delivery(&self) -> WorkerHealthHandle {
        self.webhook_delivery.clone()
    }

    /// Return the target-probe worker handle.
    pub fn target_probe(&self) -> WorkerHealthHandle {
        self.target_probe.clone()
    }

    /// Return the monitor-overdue worker handle.
    pub fn monitor_overdue(&self) -> WorkerHealthHandle {
        self.monitor_overdue.clone()
    }

    /// Return the retention-prune worker handle.
    pub fn retention_prune(&self) -> WorkerHealthHandle {
        self.retention_prune.clone()
    }

    /// Return the TLS-expiry scan worker handle.
    pub fn tls_expiry_scan(&self) -> WorkerHealthHandle {
        self.tls_expiry_scan.clone()
    }

    /// Return snapshots in stable worker order.
    pub fn snapshot(&self) -> Vec<WorkerReadyzCheck> {
        self.snapshot_at(0)
    }

    /// Return snapshots in stable worker order with last-success ages relative to now.
    pub fn snapshot_at(&self, now_unix_ms: i64) -> Vec<WorkerReadyzCheck> {
        LIFECYCLE_WORKERS
            .into_iter()
            .map(|name| match name {
                WorkerName::WebhookDelivery => self.webhook_delivery.snapshot_at(now_unix_ms),
                WorkerName::TargetProbe => self.target_probe.snapshot_at(now_unix_ms),
                WorkerName::MonitorOverdue => self.monitor_overdue.snapshot_at(now_unix_ms),
                WorkerName::RetentionPrune => self.retention_prune.snapshot_at(now_unix_ms),
                WorkerName::TlsExpiryScan => self.tls_expiry_scan.snapshot_at(now_unix_ms),
            })
            .collect()
    }
}

impl Default for WorkerHealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Worker-local health recorder.
#[derive(Debug, Clone)]
pub struct WorkerHealthHandle {
    inner: Arc<WorkerHealthInner>,
}

/// Last-observed work pressure for a lifecycle pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerPressureSnapshot {
    /// Work items due at the last observed lifecycle pass.
    pub due_count: u64,
    /// Work items still in flight at the last observed lifecycle pass.
    pub in_flight_count: u64,
    /// Milliseconds by which the oldest due work item is overdue, when known.
    pub oldest_due_age_ms: Option<u64>,
    /// Identifying metadata for the oldest due work item, when known.
    pub oldest_due_item: Option<WorkerDueItem>,
    /// Whether the pass saw backoff, circuit-open, or interruption pressure.
    pub backoff_or_circuit_open: bool,
}

impl WorkerHealthHandle {
    /// Build an isolated health handle.
    pub fn new(name: WorkerName) -> Self {
        Self {
            inner: Arc::new(WorkerHealthInner {
                name,
                started: AtomicBool::new(false),
                stopped: AtomicBool::new(true),
                failure_count: AtomicU64::new(0),
                consecutive_failures: AtomicU64::new(0),
                last_success_unix_ms: AtomicI64::new(-1),
                details: Mutex::new(WorkerHealthDetails::default()),
            }),
        }
    }

    /// Mark the worker thread as running.
    pub fn mark_started(&self) {
        self.inner.started.store(true, Ordering::SeqCst);
        self.inner.stopped.store(false, Ordering::SeqCst);
    }

    /// Mark the worker thread as stopped.
    pub fn mark_stopped(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
    }

    /// Record one successful lifecycle pass and its pressure summary.
    pub fn record_success_with_pressure(
        &self,
        observed_at: String,
        observed_unix_ms: i64,
        pressure: WorkerPressureSnapshot,
    ) {
        self.mark_started();
        self.inner.consecutive_failures.store(0, Ordering::SeqCst);
        if observed_unix_ms >= 0 {
            self.inner
                .last_success_unix_ms
                .store(observed_unix_ms, Ordering::SeqCst);
        }
        if let Ok(mut details) = self.inner.details.lock() {
            details.last_success_at = Some(observed_at);
            details.pressure = pressure;
        }
    }

    /// Record a runtime failure class without storing sensitive error text.
    pub fn record_failure(&self, error_class: &'static str) {
        self.mark_started();
        self.inner.failure_count.fetch_add(1, Ordering::SeqCst);
        self.inner
            .consecutive_failures
            .fetch_add(1, Ordering::SeqCst);
        if let Ok(mut details) = self.inner.details.lock() {
            details.last_error_class = Some(error_class.to_owned());
        }
    }

    /// Return the public readiness snapshot for this worker.
    pub fn snapshot(&self) -> WorkerReadyzCheck {
        self.snapshot_at(0)
    }

    /// Return the public readiness snapshot for this worker relative to a live clock.
    pub fn snapshot_at(&self, now_unix_ms: i64) -> WorkerReadyzCheck {
        let state = if self.inner.started.load(Ordering::SeqCst)
            && !self.inner.stopped.load(Ordering::SeqCst)
        {
            WorkerLifecycleState::Started
        } else {
            WorkerLifecycleState::Stopped
        };
        let details = self
            .inner
            .details
            .lock()
            .map(|details| details.clone())
            .unwrap_or_default();
        let failure_count = self.inner.failure_count.load(Ordering::SeqCst);
        let consecutive_failures = self.inner.consecutive_failures.load(Ordering::SeqCst);
        let last_success_unix_ms = self.inner.last_success_unix_ms.load(Ordering::SeqCst);
        let last_success_age_ms = age_ms(now_unix_ms, last_success_unix_ms);
        let health = health_status(
            self.inner.name,
            state,
            last_success_age_ms,
            consecutive_failures,
            &details.pressure,
        );

        WorkerReadyzCheck {
            name: self.inner.name.as_str().to_owned(),
            state,
            health,
            last_success_at: details.last_success_at,
            last_success_age_ms,
            failure_count,
            consecutive_failures,
            last_error_class: details.last_error_class,
            pressure_shape: self.inner.name.pressure_shape(),
            due_count: details.pressure.due_count,
            in_flight_count: details.pressure.in_flight_count,
            oldest_due_age_ms: details.pressure.oldest_due_age_ms,
            oldest_due_item: details.pressure.oldest_due_item,
            backoff_or_circuit_open: details.pressure.backoff_or_circuit_open,
        }
    }
}

#[derive(Debug)]
struct WorkerHealthInner {
    name: WorkerName,
    started: AtomicBool,
    stopped: AtomicBool,
    failure_count: AtomicU64,
    consecutive_failures: AtomicU64,
    last_success_unix_ms: AtomicI64,
    details: Mutex<WorkerHealthDetails>,
}

#[derive(Debug, Clone, Default)]
struct WorkerHealthDetails {
    last_success_at: Option<String>,
    last_error_class: Option<String>,
    pressure: WorkerPressureSnapshot,
}

fn age_ms(now_unix_ms: i64, then_unix_ms: i64) -> Option<u64> {
    if now_unix_ms < 0 || then_unix_ms < 0 {
        return None;
    }
    Some(now_unix_ms.saturating_sub(then_unix_ms).max(0) as u64)
}

fn health_status(
    name: WorkerName,
    state: WorkerLifecycleState,
    last_success_age_ms: Option<u64>,
    consecutive_failures: u64,
    pressure: &WorkerPressureSnapshot,
) -> WorkerHealthStatus {
    if !matches!(state, WorkerLifecycleState::Started) {
        return WorkerHealthStatus::Stopped;
    }
    if consecutive_failures >= 3 {
        return WorkerHealthStatus::Failing;
    }
    match last_success_age_ms {
        Some(age) if age > name.stale_after_ms() as u64 => return WorkerHealthStatus::Stale,
        None => return WorkerHealthStatus::Stale,
        _ => {}
    }
    if pressure.backoff_or_circuit_open
        || pressure
            .oldest_due_age_ms
            .is_some_and(|age| age > name.pressure_after_ms())
    {
        return WorkerHealthStatus::Pressured;
    }
    WorkerHealthStatus::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_shape_marks_sweep_workers_distinctly_from_queue_workers() {
        assert_eq!(
            WorkerName::WebhookDelivery.pressure_shape(),
            WorkerPressureShape::Queue
        );
        assert_eq!(
            WorkerName::TargetProbe.pressure_shape(),
            WorkerPressureShape::Queue
        );
        assert_eq!(
            WorkerName::MonitorOverdue.pressure_shape(),
            WorkerPressureShape::Queue
        );
        assert_eq!(
            WorkerName::RetentionPrune.pressure_shape(),
            WorkerPressureShape::SweepResult
        );
        assert_eq!(
            WorkerName::TlsExpiryScan.pressure_shape(),
            WorkerPressureShape::SweepResult
        );
    }

    /// Regression for canary-911: a live audit read `retention_prune`'s
    /// `due_count: 518` and a ~19.5h success gap as a stuck 518-item backlog.
    /// The worker was healthy the whole time — `due_count` for a sweep worker is
    /// last-pass volume, not a backlog, and its cadence is 24h (stale_after is 25h).
    /// This pins both halves: a large due_count from a completed pass never taints
    /// health, and the wire snapshot is now self-describing via `pressure_shape` so
    /// a consumer cannot repeat that misread.
    #[test]
    fn retention_prune_large_due_count_from_completed_pass_is_not_pressure() {
        let worker = WorkerHealthHandle::new(WorkerName::RetentionPrune);
        worker.record_success_with_pressure(
            "2026-07-04T23:29:18Z".to_owned(),
            0,
            WorkerPressureSnapshot {
                due_count: 518,
                in_flight_count: 0,
                oldest_due_age_ms: None,
                oldest_due_item: None,
                backoff_or_circuit_open: false,
            },
        );

        // ~19.5h later: well inside the 24h tick cadence and the 25h stale threshold.
        let snapshot = worker.snapshot_at(19 * 3_600_000 + 1_800_000);

        assert_eq!(snapshot.health, WorkerHealthStatus::Ok);
        assert_eq!(snapshot.due_count, 518);
        assert_eq!(snapshot.pressure_shape, WorkerPressureShape::SweepResult);
    }

    /// A genuinely wedged retention_prune (no successful pass past its own 25h
    /// stale threshold) must still surface as unhealthy — the sweep-vs-queue
    /// distinction changes how `due_count` is read, not whether staleness fires.
    #[test]
    fn retention_prune_genuine_staleness_past_25h_still_surfaces_as_stale() {
        let worker = WorkerHealthHandle::new(WorkerName::RetentionPrune);
        worker.record_success_with_pressure(
            "2026-07-03T23:29:18Z".to_owned(),
            0,
            WorkerPressureSnapshot {
                due_count: 518,
                ..WorkerPressureSnapshot::default()
            },
        );

        let snapshot = worker.snapshot_at(25 * 3_600_000 + 1);

        assert_eq!(snapshot.health, WorkerHealthStatus::Stale);
    }

    #[test]
    fn registry_returns_all_lifecycle_workers_in_stable_order() {
        let registry = WorkerHealthRegistry::new();

        let names = registry
            .snapshot()
            .into_iter()
            .map(|worker| worker.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "webhook_delivery",
                "target_probe",
                "monitor_overdue",
                "retention_prune",
                "tls_scan"
            ]
        );
    }

    #[test]
    fn worker_handle_tracks_started_success_failure_and_stopped_state() {
        let worker = WorkerHealthHandle::new(WorkerName::WebhookDelivery);

        assert_eq!(worker.snapshot().state, WorkerLifecycleState::Stopped);

        worker.mark_started();
        worker.record_success_with_pressure(
            "2026-06-12T20:00:00Z".to_owned(),
            1_000,
            WorkerPressureSnapshot {
                due_count: 2,
                in_flight_count: 1,
                oldest_due_age_ms: Some(250),
                oldest_due_item: None,
                backoff_or_circuit_open: false,
            },
        );
        worker.record_failure("panic");
        let started = worker.snapshot_at(1_500);

        assert_eq!(started.state, WorkerLifecycleState::Started);
        assert_eq!(started.health, WorkerHealthStatus::Ok);
        assert_eq!(
            started.last_success_at.as_deref(),
            Some("2026-06-12T20:00:00Z")
        );
        assert_eq!(started.last_success_age_ms, Some(500));
        assert_eq!(started.failure_count, 1);
        assert_eq!(started.consecutive_failures, 1);
        assert_eq!(started.last_error_class.as_deref(), Some("panic"));
        assert_eq!(started.due_count, 2);
        assert_eq!(started.in_flight_count, 1);
        assert_eq!(started.oldest_due_age_ms, Some(250));

        worker.mark_stopped();
        let stopped = worker.snapshot_at(1_500);
        assert_eq!(stopped.state, WorkerLifecycleState::Stopped);
        assert_eq!(stopped.health, WorkerHealthStatus::Stopped);
    }

    #[test]
    fn worker_health_marks_stale_repeated_failures_and_pressure_not_ready() {
        let worker = WorkerHealthHandle::new(WorkerName::TargetProbe);
        worker.record_success_with_pressure(
            "2026-06-12T20:00:00Z".to_owned(),
            1_000,
            WorkerPressureSnapshot::default(),
        );
        assert_eq!(worker.snapshot_at(1_500).health, WorkerHealthStatus::Ok);
        assert_eq!(worker.snapshot_at(31_001).health, WorkerHealthStatus::Stale);

        worker.record_success_with_pressure(
            "2026-06-12T20:00:31Z".to_owned(),
            31_000,
            WorkerPressureSnapshot {
                oldest_due_age_ms: Some(121_000),
                ..WorkerPressureSnapshot::default()
            },
        );
        assert_eq!(
            worker.snapshot_at(31_500).health,
            WorkerHealthStatus::Pressured
        );

        worker.record_success_with_pressure(
            "2026-06-12T20:00:31Z".to_owned(),
            31_000,
            WorkerPressureSnapshot {
                backoff_or_circuit_open: true,
                ..WorkerPressureSnapshot::default()
            },
        );
        assert_eq!(
            worker.snapshot_at(31_500).health,
            WorkerHealthStatus::Pressured
        );

        worker.record_success_with_pressure(
            "2026-06-12T20:00:32Z".to_owned(),
            32_000,
            WorkerPressureSnapshot::default(),
        );
        worker.record_failure("runtime_error");
        worker.record_failure("runtime_error");
        assert_eq!(worker.snapshot_at(32_500).health, WorkerHealthStatus::Ok);
        worker.record_failure("runtime_error");
        assert_eq!(
            worker.snapshot_at(32_500).health,
            WorkerHealthStatus::Failing
        );
    }
}
