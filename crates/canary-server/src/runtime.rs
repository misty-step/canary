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
    sync::{Arc, Mutex},
    thread,
    time::Duration as StdDuration,
};

use axum::Router;
use canary_http::public::DependencyStatus;
use canary_ingest::{IngestConfig, IngestEffect};
use canary_store::{IncidentCorrelation, Store};
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
    server_time::{current_rfc3339, current_unix_millis},
};

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
        let store = Arc::new(Mutex::new(store));
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
            .with_allow_private_targets(allow_private_targets);
        let readiness = PublicReadiness::from_probe(Arc::new(StoreReadinessProbe {
            store: store.clone(),
            workers: worker_health.clone(),
        }));
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

struct StoreReadinessProbe {
    store: Arc<Mutex<Store>>,
    workers: WorkerHealthRegistry,
}

impl PublicReadinessProbe for StoreReadinessProbe {
    fn snapshot(&self) -> PublicReadinessSnapshot {
        let database = match self
            .store
            .lock()
            .map_err(|_| ())
            .and_then(|store| store.readiness_check().map_err(|_| ()))
        {
            Ok(()) => DependencyStatus::Ok,
            Err(()) => DependencyStatus::Error,
        };
        PublicReadinessSnapshot::with_workers(
            database,
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
    store: Arc<Mutex<Store>>,
    webhook_sink: Arc<WebhookEnqueueEffectSink>,
}

impl RuntimeIngestEffectSink {
    /// Build the runtime effect sink from explicit persistence and webhook boundaries.
    pub fn new(store: Arc<Mutex<Store>>, webhook_sink: Arc<WebhookEnqueueEffectSink>) -> Self {
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
            let mut store = self
                .store
                .lock()
                .map_err(|_| "store lock poisoned".to_owned())?;
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
