//! Process-local lifecycle health for background workers.
//!
//! Worker health is runtime state, not durable product data. This module keeps
//! the mutable counters in memory and exposes a narrow redacted snapshot for
//! readiness responses.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use canary_http::public::{WorkerLifecycleState, WorkerReadyzCheck};

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
        LIFECYCLE_WORKERS
            .into_iter()
            .map(|name| match name {
                WorkerName::WebhookDelivery => self.webhook_delivery.snapshot(),
                WorkerName::TargetProbe => self.target_probe.snapshot(),
                WorkerName::MonitorOverdue => self.monitor_overdue.snapshot(),
                WorkerName::RetentionPrune => self.retention_prune.snapshot(),
                WorkerName::TlsExpiryScan => self.tls_expiry_scan.snapshot(),
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

impl WorkerHealthHandle {
    /// Build an isolated health handle.
    pub fn new(name: WorkerName) -> Self {
        Self {
            inner: Arc::new(WorkerHealthInner {
                name,
                started: AtomicBool::new(false),
                stopped: AtomicBool::new(true),
                failure_count: AtomicU64::new(0),
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

    /// Record one successful lifecycle pass.
    pub fn record_success(&self, observed_at: String) {
        self.mark_started();
        if let Ok(mut details) = self.inner.details.lock() {
            details.last_success_at = Some(observed_at);
        }
    }

    /// Record a runtime failure class without storing sensitive error text.
    pub fn record_failure(&self, error_class: &'static str) {
        self.mark_started();
        self.inner.failure_count.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut details) = self.inner.details.lock() {
            details.last_error_class = Some(error_class.to_owned());
        }
    }

    /// Return the public readiness snapshot for this worker.
    pub fn snapshot(&self) -> WorkerReadyzCheck {
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

        WorkerReadyzCheck {
            name: self.inner.name.as_str().to_owned(),
            state,
            last_success_at: details.last_success_at,
            failure_count: self.inner.failure_count.load(Ordering::SeqCst),
            last_error_class: details.last_error_class,
        }
    }
}

#[derive(Debug)]
struct WorkerHealthInner {
    name: WorkerName,
    started: AtomicBool,
    stopped: AtomicBool,
    failure_count: AtomicU64,
    details: Mutex<WorkerHealthDetails>,
}

#[derive(Debug, Clone, Default)]
struct WorkerHealthDetails {
    last_success_at: Option<String>,
    last_error_class: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        worker.record_success("2026-06-12T20:00:00Z".to_owned());
        worker.record_failure("panic");
        let started = worker.snapshot();

        assert_eq!(started.state, WorkerLifecycleState::Started);
        assert_eq!(
            started.last_success_at.as_deref(),
            Some("2026-06-12T20:00:00Z")
        );
        assert_eq!(started.failure_count, 1);
        assert_eq!(started.last_error_class.as_deref(), Some("panic"));

        worker.mark_stopped();
        assert_eq!(worker.snapshot().state, WorkerLifecycleState::Stopped);
    }
}
