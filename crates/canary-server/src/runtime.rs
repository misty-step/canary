//! Top-level Canary process runtime composition.
//!
//! This module owns storage boot, worker wiring, and post-commit process
//! effects. The crate root keeps route registration and route state so the
//! public HTTP table remains easy for agents to inspect.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    future::Future,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::Duration as StdDuration,
};

use axum::Router;
use canary_http::public::{CANARY_VERSION, DependencyStatus, OPENAPI_JSON, stamp_openapi_version};
use canary_ingest::{IngestConfig, IngestEffect};
use canary_store::{IncidentCorrelation, ReadPool, Store};
use canary_workers::retention::RetentionPolicy;

use crate::{
    AuthFailIdentityConfig, EnqueueFailureKey, EnqueueFailureRecorder, HealthEventFanout,
    HttpWebhookTransport, InMemoryWebhookCircuit, InMemoryWebhookCooldown, IngestEffectSink,
    IngestState, MonitorOverdueLifecycle, MonitorOverdueLifecycleConfig,
    MonitorOverdueLifecycleWorker, MonitorOverdueRuntime, PublicReadiness, PublicReadinessProbe,
    PublicReadinessSnapshot, ReqwestProbeTransport, RetentionPruneLifecycle,
    RetentionPruneLifecycleConfig, RetentionPruneLifecycleWorker, StoreWebhookScheduler,
    TargetProbeLifecycle, TargetProbeLifecycleConfig, TargetProbeLifecycleWorker,
    TargetProbeOptions, TargetProbeRuntime, TlsExpiryScanLifecycle, TlsExpiryScanLifecycleConfig,
    TlsExpiryScanLifecycleWorker, WebhookDeliveryDrain, WebhookDeliveryDrainWorker,
    WebhookDeliveryRuntime, WebhookEnqueueEffectSink, WebhookTransport, WorkerHealthRegistry,
    dashboard_router, ingest_router, public_router,
    route_state::SharedStore,
    server_time::{current_rfc3339, current_unix_millis},
};

/// Bounded wait for the writer lock before `/readyz` falls back to the last
/// known-good verdict instead of queueing. Well under Fly's 5s health-check
/// timeout (`fly.toml`), so a busy writer can never itself starve this probe
/// into a restart-spiral timeout (canary-930).
const READINESS_LOCK_WAIT: StdDuration = StdDuration::from_millis(200);

/// Consecutive lock-wait timeouts tolerated before a queued writer is
/// treated as wedged rather than merely busy. Readiness stays live -- a
/// permanently stuck lock must still fail `/readyz` -- but one or two
/// timeouts under a write burst must not flip a healthy process to
/// `not_ready`.
const MAX_CONSECUTIVE_LOCK_TIMEOUTS: u32 = 3;

const DEFAULT_WEBHOOK_DRAIN_INTERVAL: StdDuration = StdDuration::from_secs(5);
const DEFAULT_WEBHOOK_DRAIN_MAX_JOBS: u32 = 25;
const DEFAULT_TARGET_PROBE_INTERVAL: StdDuration = StdDuration::from_secs(1);
const DEFAULT_MONITOR_OVERDUE_INTERVAL: StdDuration = StdDuration::from_secs(1);
const DEFAULT_RETENTION_PRUNE_INTERVAL: StdDuration = StdDuration::from_secs(24 * 60 * 60);
const DEFAULT_TLS_EXPIRY_SCAN_INTERVAL: StdDuration = StdDuration::from_secs(24 * 60 * 60);

/// Runtime configuration for the top-level Canary server process.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// SQLite database path opened by the single-writer store.
    pub database_path: PathBuf,
    /// Ingest-domain limits and defaults.
    pub ingest: IngestConfig,
    /// Interval for the dedicated webhook delivery drain thread.
    pub webhook_drain_interval: StdDuration,
    /// Maximum scheduled webhook jobs claimed by one drain pass.
    pub webhook_drain_max_jobs: u32,
    /// Minimum interval between active target probe lifecycle passes.
    pub target_probe_interval: StdDuration,
    /// Runtime options for HTTP target probes.
    pub target_probe_options: TargetProbeOptions,
    /// Minimum interval between non-HTTP monitor overdue evaluation passes.
    pub monitor_overdue_interval: StdDuration,
    /// Minimum interval between retention prune passes.
    pub retention_prune_interval: StdDuration,
    /// Minimum interval between persisted TLS-expiry scan passes.
    pub tls_expiry_scan_interval: StdDuration,
    /// Retention policy used by the maintenance prune worker.
    pub retention_policy: RetentionPolicy,
    /// Client identity source for silent invalid-key accounting.
    pub auth_fail_identity: AuthFailIdentityConfig,
    /// Whether first boot should disclose the one-time bootstrap key on stderr.
    pub disclose_bootstrap_key: bool,
    /// Test-only outbound webhook transport builder.
    #[cfg(test)]
    pub webhook_transport_builder: fn() -> Result<Arc<dyn WebhookTransport>, String>,
}

impl ServerConfig {
    /// Build a server configuration from an explicit SQLite database path.
    pub fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
            ingest: IngestConfig::default(),
            webhook_drain_interval: DEFAULT_WEBHOOK_DRAIN_INTERVAL,
            webhook_drain_max_jobs: DEFAULT_WEBHOOK_DRAIN_MAX_JOBS,
            target_probe_interval: DEFAULT_TARGET_PROBE_INTERVAL,
            target_probe_options: TargetProbeOptions::default(),
            monitor_overdue_interval: DEFAULT_MONITOR_OVERDUE_INTERVAL,
            retention_prune_interval: DEFAULT_RETENTION_PRUNE_INTERVAL,
            tls_expiry_scan_interval: DEFAULT_TLS_EXPIRY_SCAN_INTERVAL,
            retention_policy: RetentionPolicy::default(),
            auth_fail_identity: AuthFailIdentityConfig::default(),
            disclose_bootstrap_key: true,
            #[cfg(test)]
            webhook_transport_builder: build_default_webhook_transport,
        }
    }
}

/// Fully wired Canary server runtime.
pub struct CanaryServer {
    router: Router,
    webhook_worker: WebhookDeliveryDrainWorker,
    target_probe_worker: TargetProbeLifecycleWorker,
    monitor_overdue_worker: MonitorOverdueLifecycleWorker,
    retention_prune_worker: RetentionPruneLifecycleWorker,
    tls_expiry_scan_worker: TlsExpiryScanLifecycleWorker,
    enqueue_failure_sink: Arc<EnqueueFailureRecorder>,
    worker_health: WorkerHealthRegistry,
}

impl CanaryServer {
    /// Open storage, run migrations, wire HTTP routes, and start webhook draining.
    pub fn boot(config: ServerConfig) -> Result<Self, ServerBootError> {
        // Fail before ever accepting a connection if the checked-in OpenAPI
        // contract can't be stamped with this build's version (for example,
        // a reformatted priv/openapi/openapi.json that moved the info.version
        // placeholder) — the alternative is silently serving a stale version
        // forever from a process that otherwise looks healthy.
        stamp_openapi_version(OPENAPI_JSON, CANARY_VERSION)
            .map_err(ServerBootError::OpenApiContract)?;

        if config.webhook_drain_max_jobs == 0 {
            return Err(ServerBootError::InvalidConfig(
                "webhook drain max jobs must be greater than zero".to_owned(),
            ));
        }
        if config.target_probe_interval.is_zero() {
            return Err(ServerBootError::InvalidConfig(
                "target probe interval must be greater than zero".to_owned(),
            ));
        }
        if config.monitor_overdue_interval.is_zero() {
            return Err(ServerBootError::InvalidConfig(
                "monitor overdue interval must be greater than zero".to_owned(),
            ));
        }
        if config.retention_prune_interval.is_zero() {
            return Err(ServerBootError::InvalidConfig(
                "retention prune interval must be greater than zero".to_owned(),
            ));
        }
        if config.tls_expiry_scan_interval.is_zero() {
            return Err(ServerBootError::InvalidConfig(
                "tls expiry scan interval must be greater than zero".to_owned(),
            ));
        }

        let mut store = Store::open(&config.database_path).map_err(ServerBootError::Store)?;
        store.migrate().map_err(ServerBootError::Store)?;
        if let Some(raw_key) = store
            .apply_initial_seed(&current_rfc3339())
            .map_err(ServerBootError::Store)?
        {
            if config.disclose_bootstrap_key {
                eprintln!("Bootstrap API key: {raw_key}");
                eprintln!("Store this key securely - it will not be shown again.");
            } else {
                eprintln!("Bootstrap API key created but not disclosed by process config.");
            }
        }
        let read_pool =
            Arc::new(ReadPool::open(&config.database_path).map_err(ServerBootError::Store)?);
        let store: SharedStore = Arc::new(parking_lot::Mutex::new(store));
        let worker_health = WorkerHealthRegistry::new();

        let scheduler = Arc::new(StoreWebhookScheduler::new(store.clone()));
        let webhook_cooldown = Arc::new(InMemoryWebhookCooldown::default());
        let webhook_circuit = Arc::new(InMemoryWebhookCircuit::default());
        let webhook_sink = Arc::new(WebhookEnqueueEffectSink::new(
            store.clone(),
            scheduler,
            webhook_cooldown,
        ));
        let effect_sink = Arc::new(RuntimeIngestEffectSink::new(
            store.clone(),
            webhook_sink.clone(),
        ));
        let enqueue_failure_sink = Arc::new(EnqueueFailureRecorder::default());
        let health_fanout =
            HealthEventFanout::new(webhook_sink.clone(), enqueue_failure_sink.clone());
        let ingest_state = IngestState::new_with_shared_fanout(
            store.clone(),
            config.ingest,
            effect_sink,
            health_fanout.clone(),
        );

        #[cfg(test)]
        let transport = (config.webhook_transport_builder)().map_err(ServerBootError::Http)?;
        #[cfg(not(test))]
        let transport = build_default_webhook_transport().map_err(ServerBootError::Http)?;
        let runtime = WebhookDeliveryRuntime::new(store.clone(), transport, webhook_circuit);
        let drain =
            WebhookDeliveryDrain::new(store.clone(), runtime, config.webhook_drain_max_jobs);
        let webhook_worker = WebhookDeliveryDrainWorker::spawn_with_health(
            drain,
            config.webhook_drain_interval,
            worker_health.webhook_delivery(),
        )
        .map_err(ServerBootError::WebhookWorker)?;
        let allow_private_targets = config.target_probe_options.allow_private_targets;
        let target_transport = Arc::new(ReqwestProbeTransport);
        let target_runtime = TargetProbeRuntime::new(
            store.clone(),
            health_fanout.clone(),
            target_transport,
            config.target_probe_options,
        );
        let target_probe_worker = TargetProbeLifecycleWorker::spawn_with_health(
            TargetProbeLifecycle::new(store.clone(), target_runtime),
            TargetProbeLifecycleConfig {
                tick_interval: config.target_probe_interval,
            },
            worker_health.target_probe(),
        )
        .map_err(ServerBootError::TargetProbeWorker)?;
        let monitor_overdue_worker = MonitorOverdueLifecycleWorker::spawn_with_health(
            MonitorOverdueLifecycle::new(
                store.clone(),
                MonitorOverdueRuntime::new(store.clone(), health_fanout),
            ),
            MonitorOverdueLifecycleConfig {
                tick_interval: config.monitor_overdue_interval,
            },
            worker_health.monitor_overdue(),
        )
        .map_err(ServerBootError::MonitorOverdueWorker)?;
        let retention_prune_worker = RetentionPruneLifecycleWorker::spawn_with_health(
            RetentionPruneLifecycle::new(store.clone(), config.retention_policy),
            RetentionPruneLifecycleConfig {
                tick_interval: config.retention_prune_interval,
            },
            worker_health.retention_prune(),
        )
        .map_err(ServerBootError::RetentionPruneWorker)?;
        let tls_expiry_scan_worker = TlsExpiryScanLifecycleWorker::spawn_with_health(
            TlsExpiryScanLifecycle::new(store.clone(), webhook_sink),
            TlsExpiryScanLifecycleConfig {
                tick_interval: config.tls_expiry_scan_interval,
            },
            worker_health.tls_expiry_scan(),
        )
        .map_err(ServerBootError::TlsExpiryScanWorker)?;
        let ingest_state = ingest_state
            .with_target_control(Arc::new(target_probe_worker.controller()))
            .with_auth_fail_identity(config.auth_fail_identity)
            .with_allow_private_targets(allow_private_targets)
            .with_read_pool(read_pool);
        let readiness = PublicReadiness::from_probe(Arc::new(StoreReadinessProbe::new(
            store.clone(),
            worker_health.clone(),
            READINESS_LOCK_WAIT,
            MAX_CONSECUTIVE_LOCK_TIMEOUTS,
        )));
        let router = public_router(readiness)
            .merge(dashboard_router())
            .merge(ingest_router(ingest_state));

        Ok(Self {
            router,
            webhook_worker,
            target_probe_worker,
            monitor_overdue_worker,
            retention_prune_worker,
            tls_expiry_scan_worker,
            enqueue_failure_sink,
            worker_health,
        })
    }

    /// Return a clone of the composed public and authenticated router.
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// Return health-transition webhook enqueue failures observed by this process.
    pub fn enqueue_failure_snapshot(&self) -> BTreeMap<EnqueueFailureKey, u64> {
        self.enqueue_failure_sink.snapshot()
    }

    /// Return retention-prune lifecycle failures observed by this process.
    pub fn retention_prune_failure_count(&self) -> u64 {
        self.retention_prune_worker.failure_count()
    }

    /// Return TLS-expiry scan lifecycle failures observed by this process.
    pub fn tls_expiry_scan_failure_count(&self) -> u64 {
        self.tls_expiry_scan_worker.failure_count()
    }

    /// Return background worker lifecycle readiness snapshots.
    pub fn worker_health_snapshot(&self) -> Vec<canary_http::public::WorkerReadyzCheck> {
        self.worker_health.snapshot()
    }

    #[cfg(test)]
    pub(crate) fn stop_webhook_delivery_worker_for_test(&self) {
        self.webhook_worker.stop();
    }

    /// Serve the composed router until `shutdown` resolves, then stop the worker.
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServerRunError> {
        let router = self.router.clone();
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(ServerRunError::Listen)?;
        self.target_probe_worker
            .join()
            .map_err(ServerRunError::TargetProbeWorker)?;
        self.monitor_overdue_worker
            .join()
            .map_err(ServerRunError::MonitorOverdueWorker)?;
        self.retention_prune_worker
            .join()
            .map_err(ServerRunError::RetentionPruneWorker)?;
        self.tls_expiry_scan_worker
            .join()
            .map_err(ServerRunError::TlsExpiryScanWorker)?;
        self.webhook_worker
            .join()
            .map_err(ServerRunError::WebhookWorker)
    }
}

/// Failure while booting the Canary server runtime.
#[derive(Debug)]
pub enum ServerBootError {
    /// Configuration is internally inconsistent.
    InvalidConfig(String),
    /// SQLite store open or migration failed.
    Store(canary_store::StoreError),
    /// HTTP webhook client initialization failed.
    Http(String),
    /// Webhook drain worker failed to start.
    WebhookWorker(String),
    /// Target probe lifecycle worker failed to start.
    TargetProbeWorker(String),
    /// Monitor overdue lifecycle worker failed to start.
    MonitorOverdueWorker(String),
    /// Retention prune lifecycle worker failed to start.
    RetentionPruneWorker(String),
    /// TLS-expiry scan lifecycle worker failed to start.
    TlsExpiryScanWorker(String),
    /// The checked-in OpenAPI contract can't be stamped with the compiled
    /// version (see `canary_http::public::stamp_openapi_version`).
    OpenApiContract(canary_http::public::OpenApiVersionStampError),
}

impl fmt::Display for ServerBootError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(error) => formatter.write_str(error),
            Self::Store(error) => write!(formatter, "store boot failed: {error}"),
            Self::Http(error) => formatter.write_str(error),
            Self::WebhookWorker(error) => formatter.write_str(error),
            Self::TargetProbeWorker(error) => formatter.write_str(error),
            Self::MonitorOverdueWorker(error) => formatter.write_str(error),
            Self::RetentionPruneWorker(error) => formatter.write_str(error),
            Self::TlsExpiryScanWorker(error) => formatter.write_str(error),
            Self::OpenApiContract(error) => write!(formatter, "openapi contract error: {error}"),
        }
    }
}

impl Error for ServerBootError {}

/// Failure while serving the Canary server runtime.
#[derive(Debug)]
pub enum ServerRunError {
    /// The Axum listener failed while serving requests.
    Listen(std::io::Error),
    /// The webhook worker did not shut down cleanly.
    WebhookWorker(String),
    /// The target probe worker did not shut down cleanly.
    TargetProbeWorker(String),
    /// The monitor overdue worker did not shut down cleanly.
    MonitorOverdueWorker(String),
    /// The retention prune worker did not shut down cleanly.
    RetentionPruneWorker(String),
    /// The TLS-expiry scan worker did not shut down cleanly.
    TlsExpiryScanWorker(String),
}

impl fmt::Display for ServerRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Listen(error) => write!(formatter, "server listen failed: {error}"),
            Self::WebhookWorker(error) => formatter.write_str(error),
            Self::TargetProbeWorker(error) => formatter.write_str(error),
            Self::MonitorOverdueWorker(error) => formatter.write_str(error),
            Self::RetentionPruneWorker(error) => formatter.write_str(error),
            Self::TlsExpiryScanWorker(error) => formatter.write_str(error),
        }
    }
}

impl Error for ServerRunError {}

/// Readiness probe for the single-writer store.
///
/// `/readyz` must prove the *writable* store is live on every request
/// (footgun: "readiness is live" -- no static or cached verdict), but it
/// must never let a busy writer queue it past Fly's health-check timeout
/// and trigger a restart spiral (canary-930, live-reproduced: a 12-concurrent
/// `/report` wave pushed one `/readyz` call to 2.822s while it queued behind
/// writer contention).
///
/// Semantics: acquire the writer lock with a bounded wait
/// (`lock_wait`). On acquisition, run the store's tiny liveness query and
/// report its real outcome -- this is the only path that reports `Error`
/// from an actual store failure, and it always resets the timeout streak.
/// On a lock-wait timeout, the writer is busy, not necessarily broken;
/// report the last known-good verdict rather than flip a healthy process to
/// `not_ready` over one queued check. A writer that stays wedged across
/// `max_consecutive_lock_timeouts` consecutive probes is treated as failed
/// even though no liveness query ever ran -- readiness must still catch a
/// permanently stuck lock, not just a temporarily busy one.
struct StoreReadinessProbe {
    store: SharedStore,
    workers: WorkerHealthRegistry,
    lock_wait: StdDuration,
    max_consecutive_lock_timeouts: u32,
    consecutive_lock_timeouts: AtomicU32,
    last_known_database_ok: AtomicBool,
}

impl StoreReadinessProbe {
    fn new(
        store: SharedStore,
        workers: WorkerHealthRegistry,
        lock_wait: StdDuration,
        max_consecutive_lock_timeouts: u32,
    ) -> Self {
        Self {
            store,
            workers,
            lock_wait,
            max_consecutive_lock_timeouts,
            consecutive_lock_timeouts: AtomicU32::new(0),
            // Optimistic until the first probe outcome, matching a freshly
            // booted process that has not yet observed a database failure.
            last_known_database_ok: AtomicBool::new(true),
        }
    }

    fn database_status(&self) -> DependencyStatus {
        let attempt = match self.store.try_lock_for(self.lock_wait) {
            Some(store) => {
                let ok = store.readiness_check().is_ok();
                drop(store);
                LockAttempt::Checked(ok)
            }
            None => LockAttempt::TimedOut,
        };
        self.classify(attempt)
    }

    /// Pure decision policy over one lock-attempt outcome, isolated from the
    /// real mutex and store so the threshold/last-known-good behavior is
    /// directly unit-testable (see `mod tests`) without needing to fabricate
    /// a genuinely broken SQLite connection.
    fn classify(&self, attempt: LockAttempt) -> DependencyStatus {
        match attempt {
            LockAttempt::Checked(ok) => {
                self.consecutive_lock_timeouts.store(0, Ordering::Release);
                self.last_known_database_ok.store(ok, Ordering::Release);
                dependency_status(ok)
            }
            LockAttempt::TimedOut => {
                let timeouts = self
                    .consecutive_lock_timeouts
                    .fetch_add(1, Ordering::AcqRel)
                    + 1;
                if timeouts >= self.max_consecutive_lock_timeouts {
                    dependency_status(false)
                } else {
                    dependency_status(self.last_known_database_ok.load(Ordering::Acquire))
                }
            }
        }
    }
}

/// Outcome of one writer-lock acquisition attempt for readiness.
enum LockAttempt {
    /// The lock was acquired within budget; the store liveness check ran and
    /// produced this result.
    Checked(bool),
    /// The lock could not be acquired within `lock_wait`.
    TimedOut,
}

fn dependency_status(ok: bool) -> DependencyStatus {
    if ok {
        DependencyStatus::Ok
    } else {
        DependencyStatus::Error
    }
}

impl PublicReadinessProbe for StoreReadinessProbe {
    fn snapshot(&self) -> PublicReadinessSnapshot {
        PublicReadinessSnapshot::with_workers(
            self.database_status(),
            DependencyStatus::Ok,
            self.workers.snapshot_at(current_unix_millis()),
        )
    }
}

fn build_default_webhook_transport() -> Result<Arc<dyn WebhookTransport>, String> {
    let transport = thread::Builder::new()
        .name("canary-webhook-transport-init".to_owned())
        .spawn(HttpWebhookTransport::try_new)
        .map_err(|error| format!("failed to spawn webhook transport initializer: {error}"))?
        .join()
        .map_err(|_| "webhook transport initializer panicked".to_owned())??;
    Ok(Arc::new(transport))
}

/// Runtime sink for ingest post-commit effects.
pub struct RuntimeIngestEffectSink {
    store: SharedStore,
    webhook_sink: Arc<WebhookEnqueueEffectSink>,
}

impl RuntimeIngestEffectSink {
    /// Build the runtime effect sink from explicit persistence and webhook boundaries.
    pub fn new(store: SharedStore, webhook_sink: Arc<WebhookEnqueueEffectSink>) -> Self {
        Self {
            store,
            webhook_sink,
        }
    }
}

impl IngestEffectSink for RuntimeIngestEffectSink {
    fn handle(&self, effects: &[IngestEffect]) -> Result<(), String> {
        let mut errors = Vec::new();
        for effect in effects {
            let result = match effect {
                IngestEffect::BroadcastNewError { .. } => Ok(()),
                IngestEffect::CorrelateIncident {
                    tenant_id,
                    project_id,
                    signal_type,
                    signal_ref,
                    service,
                } => {
                    self.correlate_incident(tenant_id, project_id, signal_type, signal_ref, service)
                }
                IngestEffect::EnqueueWebhook {
                    event,
                    payload_json,
                } => self.webhook_sink.enqueue_event(event, payload_json),
            };

            if let Err(error) = result {
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

impl RuntimeIngestEffectSink {
    fn correlate_incident(
        &self,
        tenant_id: &str,
        project_id: &str,
        signal_type: &str,
        signal_ref: &str,
        service: &str,
    ) -> Result<(), String> {
        let event = {
            let mut store = self.store.lock();
            store
                .correlate_incident(IncidentCorrelation {
                    tenant_id: tenant_id.to_owned(),
                    project_id: project_id.to_owned(),
                    signal_type: signal_type.to_owned(),
                    signal_ref: signal_ref.to_owned(),
                    service: service.to_owned(),
                    incident_id: canary_core::ids::IncidentId::generate(),
                    event_id: canary_core::ids::EventId::generate(),
                    now: current_rfc3339(),
                })
                .map_err(|error| error.to_string())?
        };

        if let Some(event) = event {
            self.webhook_sink
                .enqueue_event(&event.event, &event.payload_json)?;
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn probe_with_policy(
        lock_wait: StdDuration,
        max_consecutive_lock_timeouts: u32,
    ) -> StoreReadinessProbe {
        let store: SharedStore = Arc::new(parking_lot::Mutex::new(
            Store::open_in_memory().expect("open in-memory store"),
        ));
        StoreReadinessProbe::new(
            store,
            WorkerHealthRegistry::new(),
            lock_wait,
            max_consecutive_lock_timeouts,
        )
    }

    // The `classify` tests below exercise the bounded-wait/consecutive-timeout
    // policy directly against synthetic `LockAttempt` outcomes. A genuinely
    // broken SQLite connection cannot be fabricated through Store's public
    // API without unsafe code (this repo's WAL-file-handle footgun means
    // even deleting the backing file leaves a live connection); `Checked(false)`
    // is the realistic proxy for "the store answered and it was broken."

    #[test]
    fn checked_success_reports_ok_and_resets_timeout_streak() {
        let probe = probe_with_policy(StdDuration::from_millis(50), 3);
        probe.consecutive_lock_timeouts.store(2, Ordering::Release);

        let status = probe.classify(LockAttempt::Checked(true));

        assert_eq!(status, DependencyStatus::Ok);
        assert_eq!(probe.consecutive_lock_timeouts.load(Ordering::Acquire), 0);
    }

    #[test]
    fn checked_failure_reports_error_immediately_regardless_of_timeout_streak() {
        let probe = probe_with_policy(StdDuration::from_millis(50), 3);

        let status = probe.classify(LockAttempt::Checked(false));

        assert_eq!(
            status,
            DependencyStatus::Error,
            "a genuine store failure must not wait for the timeout streak"
        );
        assert_eq!(probe.consecutive_lock_timeouts.load(Ordering::Acquire), 0);
    }

    #[test]
    fn timeouts_return_last_known_good_until_the_threshold_then_fail() {
        let probe = probe_with_policy(StdDuration::from_millis(50), 3);

        // Last known-good starts true (optimistic boot default).
        assert_eq!(probe.classify(LockAttempt::TimedOut), DependencyStatus::Ok);
        assert_eq!(probe.classify(LockAttempt::TimedOut), DependencyStatus::Ok);
        // Third consecutive timeout crosses the threshold: a queued writer
        // graduates from "busy" to "wedged".
        assert_eq!(
            probe.classify(LockAttempt::TimedOut),
            DependencyStatus::Error
        );
    }

    #[test]
    fn timeouts_return_last_known_bad_when_the_prior_check_failed() {
        let probe = probe_with_policy(StdDuration::from_millis(50), 3);
        assert_eq!(
            probe.classify(LockAttempt::Checked(false)),
            DependencyStatus::Error
        );

        assert_eq!(
            probe.classify(LockAttempt::TimedOut),
            DependencyStatus::Error
        );
    }

    #[test]
    fn a_checked_outcome_after_timeouts_recovers_readiness() {
        let probe = probe_with_policy(StdDuration::from_millis(50), 3);
        probe.classify(LockAttempt::TimedOut);
        probe.classify(LockAttempt::TimedOut);

        let status = probe.classify(LockAttempt::Checked(true));

        assert_eq!(status, DependencyStatus::Ok);
        assert_eq!(probe.consecutive_lock_timeouts.load(Ordering::Acquire), 0);
    }

    /// Live lock contention through the real mutex and store: proves
    /// `try_lock_for` wiring stays within `lock_wait` and reports the
    /// last known-good verdict while a writer briefly holds the lock,
    /// satisfying the "readyz p99 stays bounded under write contention"
    /// oracle at the unit level.
    #[test]
    fn database_status_stays_within_budget_and_ok_while_writer_is_transiently_busy() {
        let store: SharedStore = Arc::new(parking_lot::Mutex::new(
            Store::open_in_memory().expect("open in-memory store"),
        ));
        let probe = StoreReadinessProbe::new(
            store.clone(),
            WorkerHealthRegistry::new(),
            StdDuration::from_millis(50),
            3,
        );

        let (acquired_tx, acquired_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let held_store = store.clone();
        let holder = thread::spawn(move || {
            let _guard = held_store.lock();
            acquired_tx.send(()).ok();
            let _ = release_rx.recv();
        });
        acquired_rx.recv().expect("holder thread acquired the lock");

        let started = std::time::Instant::now();
        let status = probe.database_status();
        let elapsed = started.elapsed();

        release_tx.send(()).expect("signal release");
        holder.join().expect("holder thread panicked");

        assert_eq!(
            status,
            DependencyStatus::Ok,
            "a transiently busy writer must report last known-good, not fail readiness"
        );
        assert!(
            elapsed < StdDuration::from_millis(200),
            "probe must stay bounded by lock_wait, took {elapsed:?}"
        );
    }

    /// A writer held across `max_consecutive_lock_timeouts` consecutive
    /// probes must eventually fail readiness -- a permanently wedged lock
    /// cannot hide behind "transient contention" forever (footgun:
    /// "readiness is live").
    #[test]
    fn database_status_fails_once_writer_stays_wedged_past_the_threshold() {
        let store: SharedStore = Arc::new(parking_lot::Mutex::new(
            Store::open_in_memory().expect("open in-memory store"),
        ));
        let probe = StoreReadinessProbe::new(
            store.clone(),
            WorkerHealthRegistry::new(),
            StdDuration::from_millis(20),
            2,
        );

        let (acquired_tx, acquired_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let held_store = store.clone();
        let holder = thread::spawn(move || {
            let _guard = held_store.lock();
            acquired_tx.send(()).ok();
            let _ = release_rx.recv();
        });
        acquired_rx.recv().expect("holder thread acquired the lock");

        assert_eq!(probe.database_status(), DependencyStatus::Ok);
        assert_eq!(probe.database_status(), DependencyStatus::Error);

        release_tx.send(()).expect("signal release");
        holder.join().expect("holder thread panicked");

        // Once released, the very next probe recovers.
        assert_eq!(probe.database_status(), DependencyStatus::Ok);
    }
}
