//! Shared runtime state for authenticated route adapters.
//!
//! The crate root owns the route table. This module owns the capabilities those
//! route adapters need: single-writer storage, ingest configuration,
//! post-commit effects, health fanout, target control, webhook transport,
//! rate-limit state, auth-failure identity, and private target policy.

use std::sync::{Arc, Mutex, MutexGuard};

use canary_ingest::{IngestConfig, IngestEffect};
use canary_store::Store;
use canary_workers::webhooks::{TransportResult, WebhookRequest};

use crate::{
    HealthEventFanout, HttpWebhookTransport, InMemoryWebhookCooldown, RateLimiter,
    TargetProbeLifecycleCommand, TargetProbeLifecycleController, WebhookEnqueueEffectSink,
    WebhookScheduler, WebhookTransport,
};

/// Sink for already-recorded service events that should fan out to webhooks.
pub trait EventSink: Send + Sync + 'static {
    /// Enqueue one event payload. Errors are advisory after the store commit.
    fn enqueue_event(&self, event: &str, payload_json: &str) -> Result<(), String>;
}

#[derive(Debug, Default)]
struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn enqueue_event(&self, _event: &str, _payload_json: &str) -> Result<(), String> {
        Ok(())
    }
}

impl EventSink for WebhookEnqueueEffectSink {
    fn enqueue_event(&self, event: &str, payload_json: &str) -> Result<(), String> {
        WebhookEnqueueEffectSink::enqueue_event(self, event, payload_json)
    }
}

/// Shared state needed by authenticated ingest routes.
#[derive(Clone)]
pub struct IngestState {
    store: Arc<Mutex<Store>>,
    config: IngestConfig,
    effect_sink: Arc<dyn IngestEffectSink>,
    health_fanout: HealthEventFanout,
    target_control: Arc<dyn TargetControlSink>,
    webhook_transport: Arc<dyn WebhookTransport>,
    rate_limiter: Arc<Mutex<RateLimiter>>,
    auth_fail_identity: AuthFailIdentityConfig,
    allow_private_targets: bool,
}

/// Client identity source used only for Phoenix-compatible invalid-key
/// accounting.
///
/// Phoenix records invalid supplied API keys against `conn.remote_ip` and
/// deliberately ignores the rate-limit result. Rust keeps the same silent
/// accounting contract while making proxy-header trust explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AuthFailIdentityConfig {
    /// Trust proxy-set client IP headers such as `fly-client-ip` and
    /// `x-forwarded-for`.
    pub trust_proxy_headers: bool,
}

impl IngestState {
    /// Build ingest state from an already-open single-writer store.
    pub fn new(store: Store, config: IngestConfig) -> Self {
        Self::new_with_effect_sink(store, config, Arc::new(NoopIngestEffectSink))
    }

    /// Build ingest state with Rust webhook enqueue wired to a scheduler.
    ///
    /// This constructor persists webhook ledger rows and calls the supplied
    /// scheduler for `EnqueueWebhook` effects. It does not implement delivery
    /// transport or retry runtime; those remain behind the scheduler boundary.
    pub fn new_with_webhook_scheduler(
        store: Store,
        config: IngestConfig,
        scheduler: Arc<dyn WebhookScheduler>,
    ) -> Self {
        let store = Arc::new(Mutex::new(store));
        let webhook_sink = Arc::new(WebhookEnqueueEffectSink::new(
            store.clone(),
            scheduler,
            Arc::new(InMemoryWebhookCooldown::default()),
        ));
        Self {
            store,
            config,
            effect_sink: webhook_sink.clone(),
            health_fanout: HealthEventFanout::new_without_failure_sink(webhook_sink),
            target_control: Arc::new(NoopTargetControlSink),
            webhook_transport: default_webhook_transport(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::default())),
            auth_fail_identity: AuthFailIdentityConfig::default(),
            allow_private_targets: false,
        }
    }

    /// Build ingest state with an explicit post-commit effect sink.
    pub fn new_with_effect_sink(
        store: Store,
        config: IngestConfig,
        effect_sink: Arc<dyn IngestEffectSink>,
    ) -> Self {
        Self::new_with_shared_effect_sink(Arc::new(Mutex::new(store)), config, effect_sink)
    }

    /// Build ingest state from a shared single-writer store and explicit effect sink.
    pub fn new_with_shared_effect_sink(
        store: Arc<Mutex<Store>>,
        config: IngestConfig,
        effect_sink: Arc<dyn IngestEffectSink>,
    ) -> Self {
        Self {
            store,
            config,
            effect_sink,
            health_fanout: HealthEventFanout::new_without_failure_sink(Arc::new(NoopEventSink)),
            target_control: Arc::new(NoopTargetControlSink),
            webhook_transport: default_webhook_transport(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::default())),
            auth_fail_identity: AuthFailIdentityConfig::default(),
            allow_private_targets: false,
        }
    }

    /// Build ingest state from shared store plus explicit ingest and event sinks.
    pub fn new_with_shared_sinks(
        store: Arc<Mutex<Store>>,
        config: IngestConfig,
        effect_sink: Arc<dyn IngestEffectSink>,
        event_sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            store,
            config,
            effect_sink,
            health_fanout: HealthEventFanout::new_without_failure_sink(event_sink),
            target_control: Arc::new(NoopTargetControlSink),
            webhook_transport: default_webhook_transport(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::default())),
            auth_fail_identity: AuthFailIdentityConfig::default(),
            allow_private_targets: false,
        }
    }

    /// Build ingest state from shared store plus explicit ingest and health fanout sinks.
    pub fn new_with_shared_fanout(
        store: Arc<Mutex<Store>>,
        config: IngestConfig,
        effect_sink: Arc<dyn IngestEffectSink>,
        health_fanout: HealthEventFanout,
    ) -> Self {
        Self {
            store,
            config,
            effect_sink,
            health_fanout,
            target_control: Arc::new(NoopTargetControlSink),
            webhook_transport: default_webhook_transport(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::default())),
            auth_fail_identity: AuthFailIdentityConfig::default(),
            allow_private_targets: false,
        }
    }

    /// Attach the target probe lifecycle control boundary used by admin routes.
    pub fn with_target_control(mut self, target_control: Arc<dyn TargetControlSink>) -> Self {
        self.target_control = target_control;
        self
    }

    /// Attach the outbound webhook transport used by the admin test route.
    pub fn with_webhook_transport(mut self, webhook_transport: Arc<dyn WebhookTransport>) -> Self {
        self.webhook_transport = webhook_transport;
        self
    }

    /// Configure the client identity source used for silent invalid-key
    /// accounting.
    pub fn with_auth_fail_identity(mut self, config: AuthFailIdentityConfig) -> Self {
        self.auth_fail_identity = config;
        self
    }

    /// Allow admin target creation to accept private/non-global probe hosts.
    pub fn with_allow_private_targets(mut self, allow_private_targets: bool) -> Self {
        self.allow_private_targets = allow_private_targets;
        self
    }

    pub(crate) fn lock_store(&self) -> Result<MutexGuard<'_, Store>, String> {
        self.store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())
    }

    #[cfg(test)]
    pub(crate) fn shared_store(&self) -> Arc<Mutex<Store>> {
        self.store.clone()
    }

    pub(crate) fn config(&self) -> &IngestConfig {
        &self.config
    }

    pub(crate) fn handle_effects(&self, effects: &[IngestEffect]) -> Result<(), String> {
        self.effect_sink.handle(effects)
    }

    pub(crate) fn health_fanout(&self) -> &HealthEventFanout {
        &self.health_fanout
    }

    pub(crate) fn control_target(
        &self,
        command: TargetProbeLifecycleCommand,
    ) -> Result<(), String> {
        self.target_control.control_target(command)
    }

    pub(crate) fn webhook_transport(&self) -> Arc<dyn WebhookTransport> {
        self.webhook_transport.clone()
    }

    pub(crate) fn rate_limiter(&self) -> &Arc<Mutex<RateLimiter>> {
        &self.rate_limiter
    }

    pub(crate) fn auth_fail_identity(&self) -> AuthFailIdentityConfig {
        self.auth_fail_identity
    }

    pub(crate) fn allow_private_targets(&self) -> bool {
        self.allow_private_targets
    }

    #[cfg(test)]
    pub(crate) fn replace_effect_sink(&mut self, effect_sink: Arc<dyn IngestEffectSink>) {
        self.effect_sink = effect_sink;
    }
}

/// Best-effort sink for ingest effects emitted after the store transaction commits.
pub trait IngestEffectSink: Send + Sync + 'static {
    /// Handle effects. Errors are advisory and must not change the HTTP response.
    fn handle(&self, effects: &[IngestEffect]) -> Result<(), String>;
}

#[derive(Debug, Default)]
struct NoopIngestEffectSink;

impl IngestEffectSink for NoopIngestEffectSink {
    fn handle(&self, _effects: &[IngestEffect]) -> Result<(), String> {
        Ok(())
    }
}

/// Narrow control boundary from admin target writes to the probe lifecycle.
pub trait TargetControlSink: Send + Sync + 'static {
    /// Apply one target-scoped lifecycle command.
    fn control_target(&self, command: TargetProbeLifecycleCommand) -> Result<(), String>;
}

#[derive(Debug, Default)]
struct NoopTargetControlSink;

impl TargetControlSink for NoopTargetControlSink {
    fn control_target(&self, _command: TargetProbeLifecycleCommand) -> Result<(), String> {
        Ok(())
    }
}

impl TargetControlSink for TargetProbeLifecycleController {
    fn control_target(&self, command: TargetProbeLifecycleCommand) -> Result<(), String> {
        TargetProbeLifecycleController::control_target(self, command)
    }
}

fn default_webhook_transport() -> Arc<dyn WebhookTransport> {
    Arc::new(LazyHttpWebhookTransport)
}

struct LazyHttpWebhookTransport;

impl WebhookTransport for LazyHttpWebhookTransport {
    fn send(&self, request: &WebhookRequest) -> TransportResult {
        let request = request.clone();
        match HttpWebhookTransport::try_new() {
            Ok(transport) => transport.send(&request),
            Err(error) => TransportResult::RequestError(error),
        }
    }
}
