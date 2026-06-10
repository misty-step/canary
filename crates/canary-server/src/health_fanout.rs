//! Best-effort health-transition webhook fanout.
//!
//! Health transitions are already committed before this module runs. Enqueue
//! failures are observable, but advisory: they never roll back the transition or
//! change an HTTP response.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use canary_store::HealthTransitionCommit;

use crate::EventSink;

/// Health-transition source that emitted webhook fanout work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HealthEventSource {
    /// HTTP target probe runtime.
    TargetProbe,
    /// Non-HTTP monitor overdue runtime.
    MonitorOverdue,
    /// Ingest check-in route.
    MonitorCheckIn,
}

/// One failed health-transition webhook enqueue attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueFailure {
    /// Source that attempted the enqueue.
    pub source: HealthEventSource,
    /// Event name that failed to enqueue.
    pub event: String,
    /// Advisory enqueue error.
    pub error: String,
}

/// Stable aggregation key for health-transition enqueue failures.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EnqueueFailureKey {
    /// Source that attempted the enqueue.
    pub source: HealthEventSource,
    /// Event name that failed to enqueue.
    pub event: String,
}

/// Sink for advisory health-transition enqueue failures.
pub trait EnqueueFailureSink: Send + Sync + 'static {
    /// Record one enqueue failure without affecting the committed transition.
    fn record(&self, failure: EnqueueFailure);
}

/// In-memory enqueue failure counter for the current server process.
#[derive(Debug, Default)]
pub struct EnqueueFailureRecorder {
    counts: Mutex<BTreeMap<EnqueueFailureKey, u64>>,
}

impl EnqueueFailureRecorder {
    /// Return a stable snapshot of observed enqueue failures.
    pub fn snapshot(&self) -> BTreeMap<EnqueueFailureKey, u64> {
        self.counts
            .lock()
            .map(|counts| counts.clone())
            .unwrap_or_default()
    }
}

impl EnqueueFailureSink for EnqueueFailureRecorder {
    fn record(&self, failure: EnqueueFailure) {
        if let Ok(mut counts) = self.counts.lock() {
            let key = EnqueueFailureKey {
                source: failure.source,
                event: failure.event,
            };
            *counts.entry(key).or_default() += 1;
        }
    }
}

#[derive(Debug, Default)]
struct NoopEnqueueFailureSink;

impl EnqueueFailureSink for NoopEnqueueFailureSink {
    fn record(&self, _failure: EnqueueFailure) {}
}

/// Summary of one advisory health-transition webhook fanout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[must_use]
pub struct EventFanoutReport {
    /// Event enqueue attempts made.
    pub attempted: usize,
    /// Event enqueue attempts accepted by the sink.
    pub enqueued: usize,
    /// Event enqueue attempts rejected by the sink.
    pub failed: usize,
    /// Advisory enqueue errors by event name.
    pub errors: Vec<String>,
}

/// Deep boundary for best-effort health-transition webhook fanout.
#[derive(Clone)]
pub struct HealthEventFanout {
    event_sink: Arc<dyn EventSink>,
    failure_sink: Arc<dyn EnqueueFailureSink>,
}

impl HealthEventFanout {
    /// Build fanout with an explicit event sink and failure recorder.
    pub fn new(event_sink: Arc<dyn EventSink>, failure_sink: Arc<dyn EnqueueFailureSink>) -> Self {
        Self {
            event_sink,
            failure_sink,
        }
    }

    /// Build fanout that ignores enqueue failures after returning a report.
    pub fn new_without_failure_sink(event_sink: Arc<dyn EventSink>) -> Self {
        Self::new(event_sink, Arc::new(NoopEnqueueFailureSink))
    }

    /// Dispatch a committed health transition and any incident event.
    pub fn dispatch(
        &self,
        source: HealthEventSource,
        transition: &HealthTransitionCommit,
    ) -> EventFanoutReport {
        let mut report = EventFanoutReport::default();
        self.dispatch_one(
            source,
            &transition.event,
            &transition.payload_json,
            &mut report,
        );
        if let Some(event) = &transition.incident_event {
            self.dispatch_one(source, &event.event, &event.payload_json, &mut report);
        }
        report
    }

    fn dispatch_one(
        &self,
        source: HealthEventSource,
        event: &str,
        payload_json: &str,
        report: &mut EventFanoutReport,
    ) {
        report.attempted += 1;
        match self.event_sink.enqueue_event(event, payload_json) {
            Ok(()) => report.enqueued += 1,
            Err(error) => {
                report.failed += 1;
                report.errors.push(format!("{event}: {error}"));
                self.failure_sink.record(EnqueueFailure {
                    source,
                    event: event.to_owned(),
                    error,
                });
            }
        }
    }
}
