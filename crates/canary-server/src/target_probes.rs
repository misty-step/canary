//! Single-target probe runtime adapter.
//!
//! Scheduling stays outside this module. This file owns the concrete runtime
//! work needed to turn one target row into one persisted probe observation:
//! SSRF validation, bounded HTTP execution, Phoenix-compatible result mapping,
//! store commit, and post-commit webhook fanout.

use std::{
    collections::BTreeMap,
    io::Read,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration as StdDuration, Instant},
};

use canary_core::health::state_machine::{Counters, HealthState, Thresholds};
use canary_store::{ActiveTargetProbeSchedule, Store, TargetProbeSnapshot};
use canary_workers::health::{
    ObservationContext, TargetProbeObservation, TargetSnapshot, plan_target_probe,
};
use reqwest::{
    Method, Url,
    header::{HeaderName, HeaderValue},
    redirect::Policy,
};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error as RustlsError, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use x509_parser::prelude::FromDer;

use crate::{
    EventFanoutReport, HealthEventFanout, HealthEventSource, current_rfc3339, current_unix_millis,
};

const MAX_PROBE_BODY_BYTES: u64 = 64 * 1024;
const MAX_CONCURRENT_TARGET_PROBES: usize = 8;
const MAX_TARGET_HEADERS: usize = 64;
const MAX_TARGET_HEADER_BYTES: usize = 8 * 1024;
const FORBIDDEN_TARGET_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "expect",
    "host",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Options for one target probe execution.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetProbeOptions {
    /// Allow private, loopback, link-local, and otherwise non-global targets.
    pub allow_private_targets: bool,
    /// Region label persisted with the target check row.
    pub region: Option<String>,
}

/// Persisted result of one target probe execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetProbeOutcome {
    /// Target id.
    pub target_id: String,
    /// Persisted probe result.
    pub result: String,
    /// Persisted target state after the probe.
    pub state: String,
    /// Persisted target-state sequence after the probe.
    pub sequence: i64,
    /// Persisted TLS certificate expiration timestamp, when captured.
    pub tls_expires_at: Option<String>,
    /// Health transition event enqueued after commit, when any.
    pub transition_event: Option<String>,
    /// Advisory webhook fanout result for the transition event.
    pub event_fanout: EventFanoutReport,
}

/// Runtime failure that prevented a probe command from being planned.
#[derive(Debug, thiserror::Error)]
pub enum TargetProbeRuntimeError {
    /// Store lock was poisoned.
    #[error("store lock poisoned")]
    StoreLock,
    /// Store returned an error.
    #[error("store error: {0}")]
    Store(#[from] canary_store::StoreError),
    /// Target does not exist or is inactive.
    #[error("target not found")]
    TargetNotFound,
    /// Target row has unsupported persisted data.
    #[error("invalid target configuration: {0}")]
    InvalidTarget(String),
}

/// HTTP response observed by the target probe transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeHttpResponse {
    /// HTTP status code.
    pub status_code: u16,
    /// Bounded response body.
    pub body: String,
    /// TLS certificate expiration timestamp, when available.
    pub tls_expires_at: Option<String>,
}

/// Error observed by the target probe transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeTransportError {
    /// The request timed out.
    Timeout,
    /// DNS lookup failed.
    Dns(String),
    /// TLS setup or verification failed.
    Tls(String),
    /// Any other connection or protocol error.
    Connection(String),
}

/// HTTP transport for target probes.
pub trait ProbeTransport: Send + Sync {
    /// Execute one already-validated probe request.
    fn probe(&self, request: ProbeRequest) -> Result<ProbeHttpResponse, ProbeTransportError>;
}

/// Concrete target probe transport backed by reqwest.
#[derive(Debug, Default)]
pub struct ReqwestProbeTransport;

/// Validated HTTP request passed to the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRequest {
    /// HTTP method.
    pub method: String,
    /// Original URL, preserving host for Host/SNI.
    pub url: String,
    /// Parsed request headers.
    pub headers: BTreeMap<String, String>,
    /// Timeout in milliseconds.
    pub timeout_ms: i64,
    /// Resolved and approved socket addresses for this probe.
    pub resolved_addrs: Vec<SocketAddr>,
}

/// Runtime boundary for executing target probes.
pub struct TargetProbeRuntime {
    store: Arc<Mutex<Store>>,
    health_fanout: HealthEventFanout,
    transport: Arc<dyn ProbeTransport>,
    options: TargetProbeOptions,
    transition_history: Mutex<BTreeMap<String, Vec<(HealthState, i64)>>>,
}

impl TargetProbeRuntime {
    /// Build a target probe runtime from explicit side-effect boundaries.
    pub fn new(
        store: Arc<Mutex<Store>>,
        health_fanout: HealthEventFanout,
        transport: Arc<dyn ProbeTransport>,
        options: TargetProbeOptions,
    ) -> Self {
        Self {
            store,
            health_fanout,
            transport,
            options,
            transition_history: Mutex::new(BTreeMap::new()),
        }
    }

    /// Execute and persist exactly one target probe.
    pub fn run_once(&self, target_id: &str) -> Result<TargetProbeOutcome, TargetProbeRuntimeError> {
        let history = self
            .transition_history
            .lock()
            .map_err(|_| TargetProbeRuntimeError::StoreLock)?
            .get(target_id)
            .cloned()
            .unwrap_or_default();
        let run = run_target_probe_once_with_history(
            &self.store,
            &self.health_fanout,
            self.transport.as_ref(),
            target_id,
            self.options.clone(),
            history,
        )?;
        if let Some((state, timestamp)) = run.history_transition {
            let mut history = self
                .transition_history
                .lock()
                .map_err(|_| TargetProbeRuntimeError::StoreLock)?;
            let target_history = history.entry(target_id.to_owned()).or_default();
            target_history.insert(0, (state, timestamp));
            retain_recent_transitions(target_history, timestamp);
        }
        Ok(run.outcome)
    }

    /// Forget in-memory transition history for a target removed from active probing.
    pub fn forget_transition_history(&self, target_id: &str) {
        if let Ok(mut history) = self.transition_history.lock() {
            history.remove(target_id);
        }
    }
}

/// Configuration for the target probe lifecycle worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetProbeLifecycleConfig {
    /// Minimum delay between lifecycle passes.
    pub tick_interval: StdDuration,
}

impl Default for TargetProbeLifecycleConfig {
    fn default() -> Self {
        Self {
            tick_interval: StdDuration::from_secs(1),
        }
    }
}

/// Summary of one lifecycle pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetProbeLifecycleReport {
    /// Active target schedules loaded from the store.
    pub loaded: usize,
    /// Due targets selected for execution.
    pub due: usize,
    /// Probes that completed and committed an observation.
    pub probed: usize,
    /// Probes skipped because the target was concurrently deactivated or deleted.
    pub skipped_missing: usize,
    /// Probes that failed before a commit could be planned.
    pub failed: usize,
    /// Advisory health-transition webhook enqueue failures.
    pub event_fanout_failed: usize,
}

/// Runtime control message for active target probe scheduling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetProbeLifecycleCommand {
    /// Add or refresh an active target in the runtime schedule.
    Track {
        /// Target id.
        target_id: String,
        /// Probe interval in milliseconds.
        interval_ms: i64,
    },
    /// Remove a target from the runtime schedule and forget its transition history.
    Untrack {
        /// Target id.
        target_id: String,
    },
    /// Pause runtime probing for a target while preserving its schedule.
    Pause {
        /// Target id.
        target_id: String,
    },
    /// Resume runtime probing for a target promptly.
    Resume {
        /// Target id.
        target_id: String,
    },
    /// Update an active target's runtime interval.
    Reconfigure {
        /// Target id.
        target_id: String,
        /// New probe interval in milliseconds.
        interval_ms: i64,
    },
}

/// Bounded lifecycle adapter for active HTTP target probes.
pub struct TargetProbeLifecycle {
    store: Arc<Mutex<Store>>,
    runtime: Arc<TargetProbeRuntime>,
    schedules: BTreeMap<String, ScheduledTarget>,
    commands: Option<Receiver<TargetProbeLifecycleCommand>>,
}

impl TargetProbeLifecycle {
    /// Build a lifecycle adapter from the shared store and probe runtime.
    pub fn new(store: Arc<Mutex<Store>>, runtime: TargetProbeRuntime) -> Self {
        Self {
            store,
            runtime: Arc::new(runtime),
            schedules: BTreeMap::new(),
            commands: None,
        }
    }

    /// Load active targets, execute due probes with bounded isolation, and update next due times.
    pub fn run_due(&mut self, now_millis: i64) -> Result<TargetProbeLifecycleReport, String> {
        self.run_due_until(now_millis, || false)
    }

    fn run_due_until<F>(
        &mut self,
        now_millis: i64,
        mut should_stop: F,
    ) -> Result<TargetProbeLifecycleReport, String>
    where
        F: FnMut() -> bool,
    {
        let active = self.load_active_schedules()?;
        self.reconcile(active, now_millis);
        self.drain_control_commands(now_millis);

        let due_targets = self
            .schedules
            .iter()
            .filter(|(_, schedule)| !schedule.paused && schedule.next_due_millis <= now_millis)
            .map(|(target_id, _)| target_id.clone())
            .collect::<Vec<_>>();

        let mut report = TargetProbeLifecycleReport {
            loaded: self.schedules.len(),
            due: due_targets.len(),
            ..TargetProbeLifecycleReport::default()
        };

        for batch in due_targets.chunks(MAX_CONCURRENT_TARGET_PROBES) {
            let (completion_sender, completion_receiver) = mpsc::channel();
            let mut pending = 0_usize;
            for target_id in batch {
                let runtime = Arc::clone(&self.runtime);
                let target_id = target_id.clone();
                let thread_target_id = target_id.clone();
                let completion_target_id = target_id.clone();
                let sender = completion_sender.clone();
                let handle = thread::Builder::new()
                    .name("canary-target-probe".to_owned())
                    .spawn(move || {
                        let result =
                            catch_unwind(AssertUnwindSafe(|| runtime.run_once(&thread_target_id)));
                        let _ = sender.send((completion_target_id, result));
                    });
                if handle.is_ok() {
                    pending += 1;
                } else {
                    report.failed += 1;
                    self.advance_target_schedule(&target_id, now_millis);
                }
            }
            drop(completion_sender);

            while pending > 0 {
                if should_stop() {
                    return Ok(report);
                }
                match completion_receiver.recv_timeout(StdDuration::from_millis(10)) {
                    Ok((target_id, Ok(result))) => {
                        self.record_probe_result(&mut report, &target_id, result, now_millis);
                        pending -= 1;
                    }
                    Ok((target_id, Err(_))) => {
                        report.failed += 1;
                        self.advance_target_schedule(&target_id, now_millis);
                        pending -= 1;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        report.failed += pending;
                        break;
                    }
                }
            }
        }

        Ok(report)
    }

    fn record_probe_result(
        &mut self,
        report: &mut TargetProbeLifecycleReport,
        target_id: &str,
        result: Result<TargetProbeOutcome, TargetProbeRuntimeError>,
        now_millis: i64,
    ) {
        match result {
            Ok(outcome) => {
                report.probed += 1;
                report.event_fanout_failed += outcome.event_fanout.failed;
            }
            Err(TargetProbeRuntimeError::TargetNotFound) => report.skipped_missing += 1,
            Err(_) => report.failed += 1,
        }
        self.advance_target_schedule(target_id, now_millis);
    }

    fn load_active_schedules(&self) -> Result<Vec<ActiveTargetProbeSchedule>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "store lock poisoned".to_owned())?;
        store
            .active_target_probe_schedules()
            .map_err(|error| error.to_string())
    }

    fn advance_target_schedule(&mut self, target_id: &str, now_millis: i64) {
        if let Some(schedule) = self.schedules.get_mut(target_id)
            && !schedule.paused
        {
            schedule.next_due_millis = next_due_millis(target_id, schedule.interval_ms, now_millis);
        }
    }

    fn reconcile(&mut self, active: Vec<ActiveTargetProbeSchedule>, now_millis: i64) {
        let mut next = BTreeMap::new();
        for target in active {
            let interval_ms = target.interval_ms.max(1_000);
            let existing = self.schedules.remove(&target.target_id);
            let paused = existing.as_ref().is_some_and(|schedule| schedule.paused);
            let next_due_millis = existing
                .filter(|schedule| schedule.interval_ms == interval_ms)
                .map(|schedule| schedule.next_due_millis)
                .unwrap_or(now_millis);
            next.insert(
                target.target_id,
                ScheduledTarget {
                    interval_ms,
                    next_due_millis,
                    paused,
                },
            );
        }
        self.schedules = next;
    }

    fn set_command_receiver(&mut self, commands: Receiver<TargetProbeLifecycleCommand>) {
        self.commands = Some(commands);
    }

    fn drain_control_commands(&mut self, now_millis: i64) {
        let Some(commands) = self.commands.take() else {
            return;
        };
        while let Ok(command) = commands.try_recv() {
            self.apply_control_command(command, now_millis);
        }
        self.commands = Some(commands);
    }

    fn apply_control_command(&mut self, command: TargetProbeLifecycleCommand, now_millis: i64) {
        match command {
            TargetProbeLifecycleCommand::Track {
                target_id,
                interval_ms,
            } => {
                let interval_ms = interval_ms.max(1_000);
                self.schedules.insert(
                    target_id,
                    ScheduledTarget {
                        interval_ms,
                        next_due_millis: now_millis,
                        paused: false,
                    },
                );
            }
            TargetProbeLifecycleCommand::Untrack { target_id } => {
                self.schedules.remove(&target_id);
                self.runtime.forget_transition_history(&target_id);
            }
            TargetProbeLifecycleCommand::Pause { target_id } => {
                if let Some(schedule) = self.schedules.get_mut(&target_id) {
                    schedule.paused = true;
                }
                self.runtime.forget_transition_history(&target_id);
            }
            TargetProbeLifecycleCommand::Resume { target_id } => {
                if let Some(schedule) = self.schedules.get_mut(&target_id) {
                    schedule.paused = false;
                    schedule.next_due_millis = schedule.next_due_millis.min(now_millis);
                }
            }
            TargetProbeLifecycleCommand::Reconfigure {
                target_id,
                interval_ms,
            } => {
                let interval_ms = interval_ms.max(1_000);
                if let Some(schedule) = self.schedules.get_mut(&target_id) {
                    schedule.interval_ms = interval_ms;
                    schedule.next_due_millis = schedule
                        .next_due_millis
                        .min(now_millis.saturating_add(interval_ms));
                }
            }
        }
    }
}

/// Dedicated OS-thread runner for target probe lifecycle passes.
pub struct TargetProbeLifecycleWorker {
    controller: TargetProbeLifecycleController,
    handle: Option<JoinHandle<()>>,
}

/// Cloneable control handle for target probe lifecycle hot updates.
#[derive(Clone)]
pub struct TargetProbeLifecycleController {
    control: Arc<LifecycleControl>,
    command_sender: Sender<TargetProbeLifecycleCommand>,
}

impl TargetProbeLifecycleWorker {
    /// Spawn one named background thread that coordinates bounded target probes.
    pub fn spawn(
        mut lifecycle: TargetProbeLifecycle,
        config: TargetProbeLifecycleConfig,
    ) -> Result<Self, String> {
        if config.tick_interval.is_zero() {
            return Err(
                "target probe lifecycle tick interval must be greater than zero".to_owned(),
            );
        }

        let control = Arc::new(LifecycleControl::default());
        let (command_sender, command_receiver) = mpsc::channel();
        lifecycle.set_command_receiver(command_receiver);
        let thread_control = control.clone();
        let handle = thread::Builder::new()
            .name("canary-target-probes".to_owned())
            .spawn(move || run_lifecycle_worker(lifecycle, config.tick_interval, thread_control))
            .map_err(|error| format!("failed to spawn target probe worker: {error}"))?;

        Ok(Self {
            controller: TargetProbeLifecycleController {
                control,
                command_sender,
            },
            handle: Some(handle),
        })
    }

    /// Return a cloneable control handle for HTTP admin routes.
    pub fn controller(&self) -> TargetProbeLifecycleController {
        self.controller.clone()
    }

    /// Send one target-scoped lifecycle control command to the worker.
    pub fn control_target(&self, command: TargetProbeLifecycleCommand) -> Result<(), String> {
        self.controller.control_target(command)
    }

    /// Pause future lifecycle passes without stopping the worker.
    pub fn pause(&self) {
        self.controller.control.pause();
    }

    /// Resume lifecycle passes and wake the worker promptly.
    pub fn resume(&self) {
        self.controller.control.resume();
    }

    /// Request shutdown without waiting for an in-flight probe to finish.
    pub fn stop(&self) {
        self.controller.control.stop();
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
            Err(_) => Err("target probe worker panicked".to_owned()),
        }
    }
}

impl TargetProbeLifecycleController {
    /// Send one target-scoped lifecycle control command to the worker.
    pub fn control_target(&self, command: TargetProbeLifecycleCommand) -> Result<(), String> {
        self.command_sender
            .send(command)
            .map_err(|_| "target probe lifecycle worker is stopped".to_owned())?;
        self.control.condvar.notify_all();
        Ok(())
    }
}

impl Drop for TargetProbeLifecycleWorker {
    fn drop(&mut self) {
        self.stop();
        let _ = self.join_handle();
    }
}

/// Validate one admin-supplied target definition against the probe boundary.
pub fn validate_target_configuration(
    url: &str,
    method: &str,
    headers: Option<&str>,
    allow_private_targets: bool,
) -> Result<(), String> {
    let url = validate_url(url)?;
    let host = url
        .host_str()
        .ok_or_else(|| "missing URL host".to_owned())?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| format!("missing port for target URL scheme {}", url.scheme()))?;
    validate_method(method)?;
    parse_headers(headers)?;
    resolve_and_validate(host, port, allow_private_targets)?;
    Ok(())
}

/// Execute and persist exactly one target probe.
pub fn run_target_probe_once(
    store: &Arc<Mutex<Store>>,
    health_fanout: &HealthEventFanout,
    transport: &dyn ProbeTransport,
    target_id: &str,
    options: TargetProbeOptions,
) -> Result<TargetProbeOutcome, TargetProbeRuntimeError> {
    Ok(run_target_probe_once_with_history(
        store,
        health_fanout,
        transport,
        target_id,
        options,
        Vec::new(),
    )?
    .outcome)
}

struct TargetProbeRun {
    outcome: TargetProbeOutcome,
    history_transition: Option<(HealthState, i64)>,
}

fn run_target_probe_once_with_history(
    store: &Arc<Mutex<Store>>,
    health_fanout: &HealthEventFanout,
    transport: &dyn ProbeTransport,
    target_id: &str,
    options: TargetProbeOptions,
    transition_history: Vec<(HealthState, i64)>,
) -> Result<TargetProbeRun, TargetProbeRuntimeError> {
    let snapshot = load_target_snapshot(store, target_id)?;
    let observed = observe_target(&snapshot, transport, &options);
    let current = load_target_snapshot(store, target_id)?;
    let target = target_snapshot(&snapshot, &current, transition_history)?;
    let previous_state = target.state;
    let context = ObservationContext {
        now: current_rfc3339(),
        now_millis: current_unix_millis(),
        event_id: canary_core::ids::EventId::generate(),
        incident_id: canary_core::ids::IncidentId::generate(),
        incident_event_id: canary_core::ids::EventId::generate(),
    };
    let now_millis = context.now_millis;
    let plan = plan_target_probe(target, observed, context);
    let response_target_id = plan.commit.target_id.clone();
    let response_result = plan.commit.check.result.clone();
    let response_state = plan.commit.state.clone();
    let response_tls_expires_at = plan.commit.check.tls_expires_at.clone();
    let commit = {
        let mut store = store
            .lock()
            .map_err(|_| TargetProbeRuntimeError::StoreLock)?;
        store.commit_target_probe(plan.commit)?
    };
    let mut event_fanout = EventFanoutReport::default();
    let transition_event = commit.transition.as_ref().map(|transition| {
        event_fanout = health_fanout.dispatch(HealthEventSource::TargetProbe, transition);
        transition.event.clone()
    });

    let outcome = TargetProbeOutcome {
        target_id: response_target_id,
        result: response_result,
        state: response_state,
        sequence: commit.sequence,
        tls_expires_at: response_tls_expires_at,
        transition_event,
        event_fanout,
    };
    let committed_state = health_state(&outcome.state)?;
    let history_transition =
        (committed_state != previous_state).then_some((committed_state, now_millis));

    Ok(TargetProbeRun {
        outcome,
        history_transition,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduledTarget {
    interval_ms: i64,
    next_due_millis: i64,
    paused: bool,
}

#[derive(Default)]
struct LifecycleControl {
    stopping: AtomicBool,
    paused: AtomicBool,
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
    mut lifecycle: TargetProbeLifecycle,
    interval: StdDuration,
    control: Arc<LifecycleControl>,
) {
    while !control.is_stopping() {
        if !control.is_paused() {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                lifecycle.run_due_until(current_unix_millis(), || control.is_stopping())
            }));
        }
        if control.wait(interval) {
            break;
        }
    }
}

impl ProbeTransport for ReqwestProbeTransport {
    fn probe(&self, request: ProbeRequest) -> Result<ProbeHttpResponse, ProbeTransportError> {
        let timeout = timeout(request.timeout_ms);
        let url = Url::parse(&request.url)
            .map_err(|error| ProbeTransportError::Connection(error.to_string()))?;
        let host = url
            .host_str()
            .ok_or_else(|| ProbeTransportError::Connection("missing URL host".to_owned()))?;
        let mut builder = reqwest::blocking::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .connect_timeout(timeout)
            .timeout(timeout);
        builder = builder.resolve_to_addrs(host, &request.resolved_addrs);
        let client = builder
            .build()
            .map_err(|error| ProbeTransportError::Connection(error.to_string()))?;
        let method = Method::from_bytes(request.method.as_bytes())
            .map_err(|error| ProbeTransportError::Connection(error.to_string()))?;
        let mut req = client.request(method, url);
        for (name, value) in request.headers {
            req = req.header(name, value);
        }

        let response = req.send().map_err(classify_reqwest_error)?;
        let status_code = response.status().as_u16();
        let mut body_reader = response.take(MAX_PROBE_BODY_BYTES + 1);
        let mut body = String::new();
        body_reader
            .read_to_string(&mut body)
            .map_err(|error| ProbeTransportError::Connection(error.to_string()))?;
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > MAX_PROBE_BODY_BYTES {
            body.truncate(usize::try_from(MAX_PROBE_BODY_BYTES).unwrap_or(usize::MAX));
        }

        Ok(ProbeHttpResponse {
            status_code,
            body,
            tls_expires_at: None,
        })
    }
}

#[derive(Debug)]
struct AcceptAnyServerCertificate;

impl ServerCertVerifier for AcceptAnyServerCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
        ]
    }
}

fn extract_tls_expiry(
    scheme: &str,
    host: &str,
    resolved_addrs: &[SocketAddr],
    timeout: StdDuration,
) -> Option<String> {
    if scheme != "https" {
        return None;
    }
    let server_name = ServerName::try_from(host.to_owned()).ok()?;
    for addr in resolved_addrs {
        if let Some(expiry) = extract_tls_expiry_from_addr(*addr, server_name.clone(), timeout) {
            return Some(expiry);
        }
    }
    None
}

fn extract_tls_expiry_from_addr(
    addr: SocketAddr,
    server_name: ServerName<'static>,
    timeout: StdDuration,
) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(&addr, timeout).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    let config =
        ClientConfig::builder_with_provider(rustls::crypto::aws_lc_rs::default_provider().into())
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .ok()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCertificate))
            .with_no_client_auth();
    let mut connection = ClientConnection::new(Arc::new(config), server_name).ok()?;

    while connection.is_handshaking() {
        connection.complete_io(&mut stream).ok()?;
    }

    let certificates = connection.peer_certificates()?;
    let leaf = certificates.first()?;
    certificate_not_after_rfc3339(leaf)
}

fn certificate_not_after_rfc3339(certificate: &CertificateDer<'_>) -> Option<String> {
    let (_, certificate) = x509_parser::certificate::X509Certificate::from_der(certificate).ok()?;
    certificate
        .validity()
        .not_after
        .to_datetime()
        .format(&Rfc3339)
        .ok()
}

fn load_target_snapshot(
    store: &Arc<Mutex<Store>>,
    target_id: &str,
) -> Result<TargetProbeSnapshot, TargetProbeRuntimeError> {
    let mut store = store
        .lock()
        .map_err(|_| TargetProbeRuntimeError::StoreLock)?;
    store
        .target_probe_snapshot_by_id(target_id)?
        .ok_or(TargetProbeRuntimeError::TargetNotFound)
}

fn observe_target(
    snapshot: &TargetProbeSnapshot,
    transport: &dyn ProbeTransport,
    options: &TargetProbeOptions,
) -> TargetProbeObservation {
    let started = Instant::now();
    let request = match probe_request(snapshot, options.allow_private_targets) {
        Ok(request) => request,
        Err(detail) => {
            return failed_observation("connection_error", detail, Some(0), options.region.clone());
        }
    };

    match transport.probe(request.clone()) {
        Ok(mut response) => {
            let latency_ms = elapsed_millis(started);
            if response.tls_expires_at.is_none() {
                response.tls_expires_at = probe_request_tls_expiry(&request);
            }
            response_observation(
                response,
                &snapshot.expected_status,
                snapshot.body_contains.as_deref(),
                Some(latency_ms),
                options.region.clone(),
            )
        }
        Err(ProbeTransportError::Timeout) => failed_observation(
            "timeout",
            format!("request timed out after {}ms", snapshot.timeout_ms),
            Some(elapsed_millis(started)),
            options.region.clone(),
        ),
        Err(ProbeTransportError::Dns(detail)) => failed_observation(
            "dns_error",
            detail,
            Some(elapsed_millis(started)),
            options.region.clone(),
        ),
        Err(ProbeTransportError::Tls(detail)) => failed_observation(
            "tls_error",
            detail,
            Some(elapsed_millis(started)),
            options.region.clone(),
        ),
        Err(ProbeTransportError::Connection(detail)) => failed_observation(
            "connection_error",
            detail,
            Some(elapsed_millis(started)),
            options.region.clone(),
        ),
    }
}

fn probe_request_tls_expiry(request: &ProbeRequest) -> Option<String> {
    let timeout = timeout(request.timeout_ms);
    let url = Url::parse(&request.url).ok()?;
    let host = url.host_str()?;
    extract_tls_expiry(url.scheme(), host, &request.resolved_addrs, timeout)
}

fn probe_request(
    snapshot: &TargetProbeSnapshot,
    allow_private_targets: bool,
) -> Result<ProbeRequest, String> {
    let url = validate_url(&snapshot.url)?;
    let host = url
        .host_str()
        .ok_or_else(|| "missing URL host".to_owned())?
        .to_owned();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| format!("missing port for target URL scheme {}", url.scheme()))?;
    let resolved_addrs = resolve_and_validate(&host, port, allow_private_targets)?;

    Ok(ProbeRequest {
        method: validate_method(&snapshot.method)?.to_owned(),
        url: snapshot.url.clone(),
        headers: parse_headers(snapshot.headers.as_deref())?,
        timeout_ms: snapshot.timeout_ms,
        resolved_addrs,
    })
}

fn validate_url(raw_url: &str) -> Result<Url, String> {
    let url = Url::parse(raw_url).map_err(|error| format!("invalid target URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("target URL scheme must be http or https".to_owned());
    }
    if url.host_str().is_none() {
        return Err("target URL must include a host".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("target URL must not include credentials".to_owned());
    }
    Ok(url)
}

fn validate_method(method: &str) -> Result<&str, String> {
    match method {
        "GET" | "HEAD" => Ok(method),
        _ => Err(format!("unsupported target probe method: {method}")),
    }
}

fn parse_headers(headers: Option<&str>) -> Result<BTreeMap<String, String>, String> {
    let Some(headers) = headers else {
        return Ok(BTreeMap::new());
    };
    let value: Value = serde_json::from_str(headers)
        .map_err(|error| format!("invalid target headers JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "target headers must be a JSON object".to_owned())?;
    if object.len() > MAX_TARGET_HEADERS {
        return Err(format!(
            "target headers exceed {MAX_TARGET_HEADERS} configured entries"
        ));
    }
    let mut parsed = BTreeMap::new();
    let mut serialized_bytes = 0_usize;
    for (name, value) in object {
        let Some(value) = value.as_str() else {
            return Err(format!("target header {name} must be a string"));
        };
        let normalized_name = validate_header_name(name)?;
        validate_header_value(&normalized_name, value)?;
        serialized_bytes = serialized_bytes
            .saturating_add(normalized_name.len())
            .saturating_add(2)
            .saturating_add(value.len())
            .saturating_add(2);
        if serialized_bytes > MAX_TARGET_HEADER_BYTES {
            return Err(format!(
                "target headers exceed {MAX_TARGET_HEADER_BYTES} serialized bytes"
            ));
        }
        if parsed
            .insert(normalized_name.clone(), value.to_owned())
            .is_some()
        {
            return Err(format!(
                "duplicate target header {normalized_name} after case normalization"
            ));
        }
    }
    Ok(parsed)
}

fn validate_header_name(name: &str) -> Result<String, String> {
    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| format!("invalid target header name: {name}"))?;
    let normalized = header_name.as_str().to_owned();
    if FORBIDDEN_TARGET_HEADERS.contains(&normalized.as_str()) {
        return Err(format!(
            "target header {normalized} is managed by Canary probe transport"
        ));
    }
    Ok(normalized)
}

fn validate_header_value(name: &str, value: &str) -> Result<(), String> {
    HeaderValue::from_str(value).map_err(|_| format!("invalid value for target header {name}"))?;
    Ok(())
}

fn resolve_and_validate(
    host: &str,
    port: u16,
    allow_private_targets: bool,
) -> Result<Vec<SocketAddr>, String> {
    if (host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost"))
        && !allow_private_targets
    {
        return Err("target host resolves to localhost".to_owned());
    }
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("target DNS resolution failed: {error}"))?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        return Err("target DNS resolution returned no addresses".to_owned());
    }
    if !allow_private_targets {
        for addr in &addrs {
            if !is_global_ip(addr.ip()) {
                return Err(format!(
                    "target resolved to non-global address {}",
                    addr.ip()
                ));
            }
        }
    }
    Ok(addrs)
}

fn response_observation(
    response: ProbeHttpResponse,
    expected_status: &str,
    body_contains: Option<&str>,
    latency_ms: Option<i64>,
    region: Option<String>,
) -> TargetProbeObservation {
    let result = evaluate_response(
        response.status_code,
        &response.body,
        expected_status,
        body_contains,
    );
    TargetProbeObservation {
        status_code: Some(i64::from(response.status_code)),
        latency_ms,
        result: result.to_owned(),
        tls_expires_at: response.tls_expires_at,
        error_detail: None,
        region,
    }
}

fn failed_observation(
    result: &str,
    detail: String,
    latency_ms: Option<i64>,
    region: Option<String>,
) -> TargetProbeObservation {
    TargetProbeObservation {
        status_code: None,
        latency_ms,
        result: result.to_owned(),
        tls_expires_at: None,
        error_detail: Some(detail),
        region,
    }
}

fn evaluate_response(
    status: u16,
    body: &str,
    expected_status: &str,
    body_contains: Option<&str>,
) -> &'static str {
    if (300..=399).contains(&status) {
        return "redirect_not_followed";
    }
    if !status_matches(status, expected_status) {
        return "status_mismatch";
    }
    if let Some(needle) = body_contains
        && !body.contains(needle)
    {
        return "body_mismatch";
    }
    "success"
}

fn status_matches(status: u16, expected: &str) -> bool {
    if let Some((low, high)) = expected.split_once('-') {
        return low
            .trim()
            .parse::<u16>()
            .ok()
            .zip(high.trim().parse::<u16>().ok())
            .is_some_and(|(low, high)| low <= status && status <= high);
    }
    if expected.contains(',') {
        return expected
            .split(',')
            .filter_map(|part| part.trim().parse::<u16>().ok())
            .any(|expected| expected == status);
    }
    expected.trim().parse::<u16>() == Ok(status)
}

fn target_snapshot(
    probed: &TargetProbeSnapshot,
    current: &TargetProbeSnapshot,
    transitions: Vec<(HealthState, i64)>,
) -> Result<TargetSnapshot, TargetProbeRuntimeError> {
    Ok(TargetSnapshot {
        id: probed.id.clone(),
        name: probed.name.clone(),
        service: probed.service.clone(),
        url: probed.url.clone(),
        state: health_state(&current.state)?,
        counters: Counters {
            consecutive_failures: current.consecutive_failures,
            consecutive_successes: current.consecutive_successes,
            transitions,
        },
        thresholds: Thresholds {
            degraded_after: probed.degraded_after,
            down_after: probed.down_after,
            up_after: probed.up_after,
        },
    })
}

fn retain_recent_transitions(transitions: &mut Vec<(HealthState, i64)>, now_millis: i64) {
    const FLAP_WINDOW_MILLIS: i64 = 10 * 60 * 1_000;
    transitions.retain(|(_, timestamp)| now_millis.saturating_sub(*timestamp) < FLAP_WINDOW_MILLIS);
    transitions.truncate(16);
}

fn health_state(value: &str) -> Result<HealthState, TargetProbeRuntimeError> {
    HealthState::parse_persisted(value).ok_or_else(|| {
        TargetProbeRuntimeError::InvalidTarget(format!("unknown health state: {value}"))
    })
}

fn classify_reqwest_error(error: reqwest::Error) -> ProbeTransportError {
    if error.is_timeout() {
        return ProbeTransportError::Timeout;
    }
    let detail = error.to_string();
    let lower = detail.to_lowercase();
    if lower.contains("dns") || lower.contains("resolve") {
        ProbeTransportError::Dns(detail)
    } else if lower.contains("tls") || lower.contains("certificate") {
        ProbeTransportError::Tls(detail)
    } else {
        ProbeTransportError::Connection(detail)
    }
}

fn timeout(timeout_ms: i64) -> StdDuration {
    let timeout_ms = u64::try_from(timeout_ms).unwrap_or(1).max(1);
    StdDuration::from_millis(timeout_ms)
}

fn elapsed_millis(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)
}

fn next_due_millis(target_id: &str, interval_ms: i64, now_millis: i64) -> i64 {
    let interval_ms = interval_ms.max(1_000);
    let jitter_range = (interval_ms / 10).max(1);
    let jitter = deterministic_jitter(target_id, jitter_range);
    now_millis.saturating_add((interval_ms + jitter).max(1_000))
}

fn deterministic_jitter(target_id: &str, jitter_range: i64) -> i64 {
    let span = jitter_range.saturating_mul(2).saturating_add(1);
    let mut hash = 0_i64;
    for byte in target_id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(i64::from(byte));
    }
    hash.rem_euclid(span) - jitter_range
}

fn is_global_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_global_ipv4(ip),
        IpAddr::V6(ip) => is_global_ipv6(ip),
    }
}

fn is_global_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, _, d] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || (a == 100 && (64..=127).contains(&b))
        || a == 0
        || a >= 224
        || (a == 169 && b == 254)
        || (a == 192 && b == 0)
        || (a == 192 && b == 0 && d == 8)
        || (a == 192 && b == 0 && d == 9)
        || (a == 192 && b == 0 && d == 10)
        || (a == 192 && b == 0 && d == 170)
        || (a == 192 && b == 0 && d == 171)
        || (a == 192 && b == 0 && d == 2)
        || (a == 198 && b == 18)
        || (a == 198 && b == 19)
        || (a == 198 && b == 51)
        || (a == 203 && b == 0))
}

fn is_global_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let first = segments[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first == 0x2001 && segments[1] == 0x0db8)
        || (segments[0..5] == [0, 0, 0, 0, 0] && segments[5] == 0xffff)
        || ip
            .to_ipv4_mapped()
            .is_some_and(|mapped| !is_global_ipv4(mapped)))
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        io::Write,
        net::TcpListener,
        sync::{
            Condvar, Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use canary_store::{TargetInsert, WebhookSubscriptionInsert};
    use rcgen::{CertificateParams, KeyPair, date_time_ymd};
    use rustls::{
        ServerConfig, ServerConnection, StreamOwned,
        pki_types::{CertificateDer, PrivateKeyDer},
    };

    use crate::EventSink;

    use super::*;

    type TlsTestServer = std::thread::JoinHandle<Result<(), String>>;

    #[derive(Debug)]
    struct StaticTransport {
        calls: AtomicUsize,
        result: Result<ProbeHttpResponse, ProbeTransportError>,
    }

    impl StaticTransport {
        fn ok(status_code: u16, body: &str) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                result: Ok(ProbeHttpResponse {
                    status_code,
                    body: body.to_owned(),
                    tls_expires_at: None,
                }),
            }
        }
    }

    impl ProbeTransport for StaticTransport {
        fn probe(&self, _request: ProbeRequest) -> Result<ProbeHttpResponse, ProbeTransportError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }
    }

    #[derive(Debug, Default)]
    struct RecordingRequestTransport {
        calls: AtomicUsize,
        last_request: StdMutex<Option<ProbeRequest>>,
    }

    impl ProbeTransport for RecordingRequestTransport {
        fn probe(&self, request: ProbeRequest) -> Result<ProbeHttpResponse, ProbeTransportError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_request.lock().map_err(|_| {
                ProbeTransportError::Connection("request lock poisoned".to_owned())
            })? = Some(request);
            Ok(ProbeHttpResponse {
                status_code: 200,
                body: "ok".to_owned(),
                tls_expires_at: None,
            })
        }
    }

    #[derive(Debug)]
    struct QueueTransport {
        calls: AtomicUsize,
        responses: StdMutex<Vec<ProbeHttpResponse>>,
    }

    impl QueueTransport {
        fn new(responses: Vec<ProbeHttpResponse>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                responses: StdMutex::new(responses),
            }
        }
    }

    impl ProbeTransport for QueueTransport {
        fn probe(&self, _request: ProbeRequest) -> Result<ProbeHttpResponse, ProbeTransportError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut responses = self.responses.lock().map_err(|_| {
                ProbeTransportError::Connection("responses lock poisoned".to_owned())
            })?;
            if responses.is_empty() {
                return Err(ProbeTransportError::Connection(
                    "no queued response".to_owned(),
                ));
            }
            Ok(responses.remove(0))
        }
    }

    struct BlockingTransport {
        calls: AtomicUsize,
        slow_started: StdMutex<Option<std::sync::mpsc::Sender<()>>>,
        slow_done: StdMutex<Option<std::sync::mpsc::Sender<()>>>,
        fast_done: StdMutex<Option<std::sync::mpsc::Sender<()>>>,
        release_slow: StdMutex<std::sync::mpsc::Receiver<()>>,
    }

    impl BlockingTransport {
        fn new(
            slow_started: std::sync::mpsc::Sender<()>,
            fast_done: std::sync::mpsc::Sender<()>,
            release_slow: std::sync::mpsc::Receiver<()>,
        ) -> Self {
            Self::new_with_slow_done(slow_started, None, fast_done, release_slow)
        }

        fn new_with_slow_done(
            slow_started: std::sync::mpsc::Sender<()>,
            slow_done: Option<std::sync::mpsc::Sender<()>>,
            fast_done: std::sync::mpsc::Sender<()>,
            release_slow: std::sync::mpsc::Receiver<()>,
        ) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                slow_started: StdMutex::new(Some(slow_started)),
                slow_done: StdMutex::new(slow_done),
                fast_done: StdMutex::new(Some(fast_done)),
                release_slow: StdMutex::new(release_slow),
            }
        }
    }

    impl ProbeTransport for BlockingTransport {
        fn probe(&self, request: ProbeRequest) -> Result<ProbeHttpResponse, ProbeTransportError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if request.url.contains("/slow") {
                if let Some(sender) = self
                    .slow_started
                    .lock()
                    .map_err(|_| ProbeTransportError::Connection("slow lock poisoned".to_owned()))?
                    .take()
                {
                    let _ = sender.send(());
                }
                self.release_slow
                    .lock()
                    .map_err(|_| {
                        ProbeTransportError::Connection("release lock poisoned".to_owned())
                    })?
                    .recv()
                    .map_err(|error| ProbeTransportError::Connection(error.to_string()))?;
                if let Some(sender) = self
                    .slow_done
                    .lock()
                    .map_err(|_| {
                        ProbeTransportError::Connection("slow-done lock poisoned".to_owned())
                    })?
                    .take()
                {
                    let _ = sender.send(());
                }
                return Ok(response(200, "ok"));
            }

            if let Some(sender) = self
                .fast_done
                .lock()
                .map_err(|_| ProbeTransportError::Connection("fast lock poisoned".to_owned()))?
                .take()
            {
                let _ = sender.send(());
            }
            Ok(response(200, "ok"))
        }
    }

    #[derive(Debug)]
    struct GatedPeakTransport {
        in_flight: AtomicUsize,
        peak: AtomicUsize,
        release: Arc<(StdMutex<bool>, Condvar)>,
    }

    impl GatedPeakTransport {
        fn new() -> Self {
            Self {
                in_flight: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                release: Arc::new((StdMutex::new(false), Condvar::new())),
            }
        }

        fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }

        fn release(&self) -> Result<(), String> {
            let (lock, condvar) = &*self.release;
            let mut released = lock
                .lock()
                .map_err(|_| "release lock poisoned".to_owned())?;
            *released = true;
            condvar.notify_all();
            Ok(())
        }
    }

    impl ProbeTransport for GatedPeakTransport {
        fn probe(&self, _request: ProbeRequest) -> Result<ProbeHttpResponse, ProbeTransportError> {
            let in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = self
                .peak
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |peak| {
                    (in_flight > peak).then_some(in_flight)
                });
            let (lock, condvar) = &*self.release;
            let released = lock
                .lock()
                .map_err(|_| ProbeTransportError::Connection("release lock poisoned".to_owned()))?;
            let _guard = condvar
                .wait_while(released, |released| !*released)
                .map_err(|_| ProbeTransportError::Connection("release wait poisoned".to_owned()))?;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(response(200, "ok"))
        }
    }

    struct DeactivatingTransport {
        store: Arc<Mutex<Store>>,
        calls: AtomicUsize,
    }

    impl DeactivatingTransport {
        fn new(store: Arc<Mutex<Store>>) -> Self {
            Self {
                store,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl ProbeTransport for DeactivatingTransport {
        fn probe(&self, _request: ProbeRequest) -> Result<ProbeHttpResponse, ProbeTransportError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut store = self
                .store
                .lock()
                .map_err(|_| ProbeTransportError::Connection("store lock poisoned".to_owned()))?;
            store
                .update_target_active("TGT-api", false)
                .map_err(|error| ProbeTransportError::Connection(error.to_string()))?;
            Ok(response(200, "ok"))
        }
    }

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
    fn response_mapping_preserves_phoenix_status_body_and_redirect_semantics() {
        assert_eq!(
            evaluate_response(302, "ok", "200", Some("ok")),
            "redirect_not_followed"
        );
        assert_eq!(evaluate_response(204, "", "200,204", None), "success");
        assert_eq!(
            evaluate_response(201, "created", "200-299", Some("missing")),
            "body_mismatch"
        );
        assert_eq!(
            evaluate_response(503, "down", "200-299", None),
            "status_mismatch"
        );
    }

    #[test]
    fn ssrf_classification_blocks_non_global_addresses_by_default() {
        for ip in [
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6("fc00::1".parse().unwrap_or(Ipv6Addr::LOCALHOST)),
            IpAddr::V6("::ffff:127.0.0.1".parse().unwrap_or(Ipv6Addr::LOCALHOST)),
        ] {
            assert!(!is_global_ip(ip), "{ip} should be blocked");
        }
        assert!(is_global_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
    }

    #[test]
    fn configured_headers_are_validated_and_normalized() -> Result<(), Box<dyn Error>> {
        let headers = parse_headers(Some(
            r#"{"X-Canary-Probe":"true","Authorization":"Bearer health-token"}"#,
        ))?;

        assert_eq!(
            headers
                .get("x-canary-probe")
                .ok_or("missing normalized custom header")?,
            "true"
        );
        assert_eq!(
            headers
                .get("authorization")
                .ok_or("missing normalized authorization header")?,
            "Bearer health-token"
        );
        Ok(())
    }

    #[test]
    fn configured_headers_reject_transport_owned_or_malformed_values() -> Result<(), Box<dyn Error>>
    {
        for (headers, expected) in [
            (
                r#"{"Host":"evil.example"}"#,
                "target header host is managed by Canary probe transport",
            ),
            (
                r#"{"Content-Length":"100"}"#,
                "target header content-length is managed by Canary probe transport",
            ),
            (
                r#"{"Expect":"100-continue"}"#,
                "target header expect is managed by Canary probe transport",
            ),
            (
                r#"{"Proxy-Connection":"keep-alive"}"#,
                "target header proxy-connection is managed by Canary probe transport",
            ),
            (
                r#"{"Bad Header":"value"}"#,
                "invalid target header name: Bad Header",
            ),
            (
                r#"{"X-Canary":"bad\r\nsplit"}"#,
                "invalid value for target header x-canary",
            ),
            (
                r#"{"X-Canary":"one","x-canary":"two"}"#,
                "duplicate target header x-canary after case normalization",
            ),
        ] {
            assert_parse_header_error(headers, expected)?;
        }
        Ok(())
    }

    #[test]
    fn configured_headers_reject_unbounded_count_or_size() -> Result<(), Box<dyn Error>> {
        let too_many = (0..=MAX_TARGET_HEADERS)
            .map(|index| format!(r#""X-Test-{index}":"ok""#))
            .collect::<Vec<_>>()
            .join(",");
        let too_many_json = format!("{{{too_many}}}");
        assert_parse_header_error(
            &too_many_json,
            "target headers exceed 64 configured entries",
        )?;

        let oversized = format!(r#"{{"X-Large":"{}"}}"#, "x".repeat(MAX_TARGET_HEADER_BYTES));
        assert_parse_header_error(&oversized, "target headers exceed 8192 serialized bytes")?;
        Ok(())
    }

    #[test]
    fn tls_expiry_capture_uses_approved_socket_address() -> Result<(), Box<dyn Error>> {
        let (certificate, private_key, expected_expiry) = tls_test_certificate()?;
        assert_eq!(
            certificate_not_after_rfc3339(&certificate),
            Some(expected_expiry.clone())
        );
        assert_eq!(
            extract_tls_expiry(
                "http",
                "localhost",
                &[SocketAddr::from(([127, 0, 0, 1], 443))],
                StdDuration::from_secs(1),
            ),
            None
        );
        assert_eq!(
            certificate_not_after_rfc3339(&CertificateDer::from(vec![0, 1, 2, 3])),
            None
        );

        let (addr, server) = spawn_tls_expiry_server(certificate, private_key)?;
        let observed = extract_tls_expiry("https", "localhost", &[addr], StdDuration::from_secs(5));
        server
            .join()
            .map_err(|_| "tls expiry test server panicked")?
            .map_err(|error| format!("tls expiry test server failed: {error}"))?;

        assert_eq!(observed, Some(expected_expiry));
        Ok(())
    }

    #[test]
    fn ssrf_block_persists_failed_probe_without_opening_transport() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/health", "up")?;
        let transport = StaticTransport::ok(200, "ok");
        let sink = Arc::new(RecordingSink::default());
        let fanout = HealthEventFanout::new_without_failure_sink(sink);

        let outcome = run_target_probe_once(
            &store,
            &fanout,
            &transport,
            "TGT-api",
            TargetProbeOptions::default(),
        )?;

        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
        assert_eq!(outcome.result, "connection_error");
        assert_eq!(outcome.state, "degraded");
        let store = store.lock().map_err(|_| "store lock poisoned")?;
        assert_eq!(store.error_count()?, 0);
        assert_eq!(
            store.webhook_deliveries(Default::default())?.len(),
            0,
            "no webhook subscriptions exist in this fixture"
        );
        Ok(())
    }

    #[test]
    fn invalid_configured_header_persists_failed_probe_without_opening_transport()
    -> Result<(), Box<dyn Error>> {
        let store = seeded_store_with_headers(
            "http://127.0.0.1/health",
            "up",
            Some(r#"{"Host":"evil.example"}"#.to_owned()),
        )?;
        let transport = StaticTransport::ok(200, "ok");
        let sink = Arc::new(RecordingSink::default());
        let fanout = HealthEventFanout::new_without_failure_sink(sink);

        let outcome = run_target_probe_once(
            &store,
            &fanout,
            &transport,
            "TGT-api",
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        )?;

        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
        assert_eq!(outcome.result, "connection_error");
        assert_eq!(outcome.state, "degraded");
        Ok(())
    }

    #[test]
    fn valid_configured_headers_reach_transport_normalized() -> Result<(), Box<dyn Error>> {
        let store = seeded_store_with_headers(
            "http://127.0.0.1/health",
            "unknown",
            Some(r#"{"X-Trace-Id":"abc","Accept":"application/json"}"#.to_owned()),
        )?;
        let transport = RecordingRequestTransport::default();
        let sink = Arc::new(RecordingSink::default());
        let fanout = HealthEventFanout::new_without_failure_sink(sink);

        let outcome = run_target_probe_once(
            &store,
            &fanout,
            &transport,
            "TGT-api",
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        )?;

        let request = transport
            .last_request
            .lock()
            .map_err(|_| "request lock poisoned")?
            .clone()
            .ok_or("missing recorded request")?;
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert_eq!(outcome.result, "success");
        assert_eq!(
            request.headers.get("x-trace-id").map(String::as_str),
            Some("abc")
        );
        assert_eq!(
            request.headers.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert!(!request.headers.contains_key("host"));
        Ok(())
    }

    #[test]
    fn successful_https_probe_captures_tls_expiry_after_transport() -> Result<(), Box<dyn Error>> {
        let (certificate, private_key, expected_expiry) = tls_test_certificate()?;
        let (addr, server) = spawn_tls_expiry_server(certificate, private_key)?;
        let store = seeded_store(
            &format!("https://localhost:{}/health", addr.port()),
            "unknown",
        )?;
        let transport = StaticTransport::ok(200, "ok");
        let sink = Arc::new(RecordingSink::default());
        let fanout = HealthEventFanout::new_without_failure_sink(sink);

        let outcome = run_target_probe_once(
            &store,
            &fanout,
            &transport,
            "TGT-api",
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        )?;
        server
            .join()
            .map_err(|_| "tls expiry test server panicked")?
            .map_err(|error| format!("tls expiry test server failed: {error}"))?;

        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert_eq!(outcome.result, "success");
        assert_eq!(outcome.tls_expires_at, Some(expected_expiry));
        Ok(())
    }

    #[test]
    fn successful_probe_commits_state_and_enqueues_transition() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/health", "unknown")?;
        {
            let mut store = store.lock().map_err(|_| "store lock poisoned")?;
            store.insert_webhook_subscription(WebhookSubscriptionInsert {
                id: "WHK-health".to_owned(),
                url: "https://example.test/hook".to_owned(),
                events: vec!["health_check.recovered".to_owned()],
                secret: "secret".to_owned(),
                active: true,
                created_at: "2026-05-28T20:00:00Z".to_owned(),
            })?;
        }
        let transport = StaticTransport::ok(200, "ok");
        let sink = Arc::new(RecordingSink::default());
        let fanout = HealthEventFanout::new_without_failure_sink(sink.clone());

        let outcome = run_target_probe_once(
            &store,
            &fanout,
            &transport,
            "TGT-api",
            TargetProbeOptions {
                allow_private_targets: true,
                region: Some("iad".to_owned()),
            },
        )?;

        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert_eq!(outcome.result, "success");
        assert_eq!(outcome.state, "up");
        assert_eq!(outcome.sequence, 1);
        assert_eq!(
            sink.events
                .lock()
                .map_err(|_| "events lock poisoned")?
                .as_slice(),
            ["health_check.recovered"]
        );
        Ok(())
    }

    #[test]
    fn transition_enqueue_failure_is_reported_without_failing_probe() -> Result<(), Box<dyn Error>>
    {
        let store = seeded_store("http://127.0.0.1/health", "unknown")?;
        let transport = StaticTransport::ok(200, "ok");
        let recorder = Arc::new(crate::EnqueueFailureRecorder::default());
        let fanout = HealthEventFanout::new(Arc::new(FailingSink), recorder.clone());

        let outcome = run_target_probe_once(
            &store,
            &fanout,
            &transport,
            "TGT-api",
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        )?;

        assert_eq!(outcome.result, "success");
        assert_eq!(outcome.state, "up");
        assert_eq!(outcome.event_fanout.failed, 1);
        assert_eq!(
            recorder.snapshot().get(&crate::EnqueueFailureKey {
                source: HealthEventSource::TargetProbe,
                event: "health_check.recovered".to_owned(),
            }),
            Some(&1)
        );
        Ok(())
    }

    #[test]
    fn lifecycle_loads_active_targets_and_runs_due_probes() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/health", "unknown")?;
        {
            let mut store = store.lock().map_err(|_| "store lock poisoned")?;
            store.insert_target(TargetInsert {
                id: "TGT-inactive".to_owned(),
                url: "http://127.0.0.1/inactive".to_owned(),
                name: "Inactive".to_owned(),
                service: "inactive".to_owned(),
                method: "GET".to_owned(),
                headers: None,
                interval_ms: 60_000,
                timeout_ms: 10_000,
                expected_status: "200".to_owned(),
                body_contains: Some("ok".to_owned()),
                degraded_after: 1,
                down_after: 3,
                up_after: 1,
                active: false,
                created_at: "2026-05-28T20:00:00Z".to_owned(),
            })?;
        }
        let transport = Arc::new(StaticTransport::ok(200, "ok"));
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store.clone(),
            HealthEventFanout::new_without_failure_sink(sink),
            transport.clone(),
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        );
        let mut lifecycle = TargetProbeLifecycle::new(store, runtime);

        let first = lifecycle.run_due(1_000)?;
        let second = lifecycle.run_due(1_000)?;

        assert_eq!(
            first,
            TargetProbeLifecycleReport {
                loaded: 1,
                due: 1,
                probed: 1,
                skipped_missing: 0,
                failed: 0,
                event_fanout_failed: 0,
            }
        );
        assert_eq!(
            second,
            TargetProbeLifecycleReport {
                loaded: 1,
                due: 0,
                probed: 0,
                skipped_missing: 0,
                failed: 0,
                event_fanout_failed: 0,
            }
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn lifecycle_isolates_fast_due_probe_from_slow_due_probe() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/slow", "unknown")?;
        {
            let mut store = store.lock().map_err(|_| "store lock poisoned")?;
            store.insert_target(TargetInsert {
                id: "TGT-worker".to_owned(),
                url: "http://127.0.0.1/fast".to_owned(),
                name: "Worker".to_owned(),
                service: "worker".to_owned(),
                method: "GET".to_owned(),
                headers: None,
                interval_ms: 60_000,
                timeout_ms: 10_000,
                expected_status: "200".to_owned(),
                body_contains: Some("ok".to_owned()),
                degraded_after: 1,
                down_after: 3,
                up_after: 1,
                active: true,
                created_at: "2026-05-28T20:00:00Z".to_owned(),
            })?;
        }
        let (slow_started_tx, slow_started_rx) = std::sync::mpsc::channel();
        let (fast_done_tx, fast_done_rx) = std::sync::mpsc::channel();
        let (release_slow_tx, release_slow_rx) = std::sync::mpsc::channel();
        let transport = Arc::new(BlockingTransport::new(
            slow_started_tx,
            fast_done_tx,
            release_slow_rx,
        ));
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store.clone(),
            HealthEventFanout::new_without_failure_sink(sink),
            transport.clone(),
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        );
        let mut lifecycle = TargetProbeLifecycle::new(store.clone(), runtime);

        let handle = thread::spawn(move || lifecycle.run_due(1_000));
        slow_started_rx.recv_timeout(StdDuration::from_secs(1))?;
        fast_done_rx.recv_timeout(StdDuration::from_secs(1))?;
        let started = Instant::now();
        loop {
            let worker = {
                let store = store.lock().map_err(|_| "store lock poisoned")?;
                store
                    .health_targets()?
                    .into_iter()
                    .find(|target| target.id == "TGT-worker")
                    .ok_or("missing worker target")?
            };
            if worker.state == "up" && worker.last_checked_at.is_some() {
                break;
            }
            if started.elapsed() > StdDuration::from_secs(1) {
                return Err(format!(
                    "fast target did not commit while slow target was blocked: {worker:?}"
                )
                .into());
            }
            thread::sleep(StdDuration::from_millis(10));
        }

        release_slow_tx.send(())?;
        let report = handle
            .join()
            .map_err(|_| "target lifecycle test thread panicked")??;

        assert_eq!(
            report,
            TargetProbeLifecycleReport {
                loaded: 2,
                due: 2,
                probed: 2,
                skipped_missing: 0,
                failed: 0,
                event_fanout_failed: 0,
            }
        );
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[test]
    fn lifecycle_caps_concurrent_due_probe_fanout() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/target-api", "unknown")?;
        {
            let mut store = store.lock().map_err(|_| "store lock poisoned")?;
            for index in 0..9 {
                store.insert_target(TargetInsert {
                    id: format!("TGT-concurrent-{index:02}"),
                    url: format!("http://127.0.0.1/target-{index:02}"),
                    name: format!("Target {index:02}"),
                    service: "api".to_owned(),
                    method: "GET".to_owned(),
                    headers: None,
                    interval_ms: 60_000,
                    timeout_ms: 10_000,
                    expected_status: "200".to_owned(),
                    body_contains: Some("ok".to_owned()),
                    degraded_after: 1,
                    down_after: 3,
                    up_after: 1,
                    active: true,
                    created_at: "2026-05-28T20:00:00Z".to_owned(),
                })?;
            }
        }
        let transport = Arc::new(GatedPeakTransport::new());
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store.clone(),
            HealthEventFanout::new_without_failure_sink(sink),
            transport.clone(),
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        );
        let mut lifecycle = TargetProbeLifecycle::new(store, runtime);

        let handle = thread::spawn(move || lifecycle.run_due(1_000));
        let started = Instant::now();
        while transport.peak() < MAX_CONCURRENT_TARGET_PROBES {
            if started.elapsed() > StdDuration::from_secs(1) {
                return Err(format!(
                    "probe fanout did not reach the configured cap; peak={}",
                    transport.peak()
                )
                .into());
            }
            thread::sleep(StdDuration::from_millis(10));
        }
        assert_eq!(transport.peak(), MAX_CONCURRENT_TARGET_PROBES);

        transport.release()?;
        let report = handle
            .join()
            .map_err(|_| "target lifecycle test thread panicked")??;

        assert_eq!(report.loaded, 10);
        assert_eq!(report.due, 10);
        assert_eq!(report.probed, 10);
        assert_eq!(transport.peak(), MAX_CONCURRENT_TARGET_PROBES);
        Ok(())
    }

    #[test]
    fn lifecycle_worker_does_not_duplicate_long_running_probe_before_completion()
    -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/slow", "unknown")?;
        let (slow_started_tx, slow_started_rx) = std::sync::mpsc::channel();
        let (fast_done_tx, _fast_done_rx) = std::sync::mpsc::channel();
        let (release_slow_tx, release_slow_rx) = std::sync::mpsc::channel();
        let transport = Arc::new(BlockingTransport::new(
            slow_started_tx,
            fast_done_tx,
            release_slow_rx,
        ));
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store.clone(),
            HealthEventFanout::new_without_failure_sink(sink),
            transport.clone(),
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        );
        let lifecycle = TargetProbeLifecycle::new(store, runtime);
        let worker = TargetProbeLifecycleWorker::spawn(
            lifecycle,
            TargetProbeLifecycleConfig {
                tick_interval: StdDuration::from_millis(10),
            },
        )?;

        slow_started_rx.recv_timeout(StdDuration::from_secs(1))?;
        thread::sleep(StdDuration::from_millis(100));
        assert_eq!(
            transport.calls.load(Ordering::SeqCst),
            1,
            "a blocked target must not be relaunched on later lifecycle ticks"
        );

        release_slow_tx.send(())?;
        worker.join()?;
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn lifecycle_worker_shutdown_does_not_wait_for_blocked_probe_transport()
    -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/slow", "unknown")?;
        let (slow_started_tx, slow_started_rx) = std::sync::mpsc::channel();
        let (slow_done_tx, slow_done_rx) = std::sync::mpsc::channel();
        let (fast_done_tx, _fast_done_rx) = std::sync::mpsc::channel();
        let (release_slow_tx, release_slow_rx) = std::sync::mpsc::channel();
        let transport = Arc::new(BlockingTransport::new_with_slow_done(
            slow_started_tx,
            Some(slow_done_tx),
            fast_done_tx,
            release_slow_rx,
        ));
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store.clone(),
            HealthEventFanout::new_without_failure_sink(sink),
            transport.clone(),
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        );
        let store_for_assert = store.clone();
        let lifecycle = TargetProbeLifecycle::new(store, runtime);
        let worker = TargetProbeLifecycleWorker::spawn(
            lifecycle,
            TargetProbeLifecycleConfig {
                tick_interval: StdDuration::from_millis(10),
            },
        )?;

        slow_started_rx.recv_timeout(StdDuration::from_secs(1))?;
        let (joined_tx, joined_rx) = std::sync::mpsc::channel();
        let join_thread = thread::spawn(move || {
            let _ = joined_tx.send(worker.join());
        });
        joined_rx
            .recv_timeout(StdDuration::from_secs(1))?
            .map_err(|error| format!("worker join failed: {error}"))?;

        release_slow_tx.send(())?;
        slow_done_rx.recv_timeout(StdDuration::from_secs(1))?;
        let started = Instant::now();
        loop {
            let target = {
                let store = store_for_assert.lock().map_err(|_| "store lock poisoned")?;
                store
                    .health_targets()?
                    .into_iter()
                    .find(|target| target.id == "TGT-api")
                    .ok_or("missing target")?
            };
            if target.state == "up" {
                break;
            }
            if started.elapsed() > StdDuration::from_secs(1) {
                return Err("detached probe did not finish after release".into());
            }
            thread::sleep(StdDuration::from_millis(10));
        }
        join_thread
            .join()
            .map_err(|_| "target lifecycle join test thread panicked")?;
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn lifecycle_reloads_interval_changes_without_duplicate_due_runs() -> Result<(), Box<dyn Error>>
    {
        let store = seeded_store("http://127.0.0.1/health", "unknown")?;
        let transport = Arc::new(StaticTransport::ok(200, "ok"));
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store.clone(),
            HealthEventFanout::new_without_failure_sink(sink),
            transport.clone(),
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        );
        let mut lifecycle = TargetProbeLifecycle::new(store, runtime);

        let first = lifecycle.run_due(1_000)?;
        let second = lifecycle.run_due(1_001)?;

        assert_eq!(first.probed, 1);
        assert_eq!(second.due, 0);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn lifecycle_target_pause_resume_commands_control_due_selection() -> Result<(), Box<dyn Error>>
    {
        let store = seeded_store("http://127.0.0.1/health", "unknown")?;
        let transport = Arc::new(QueueTransport::new(vec![
            response(200, "ok"),
            response(200, "ok"),
        ]));
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store.clone(),
            HealthEventFanout::new_without_failure_sink(sink),
            transport.clone(),
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        );
        let mut lifecycle = TargetProbeLifecycle::new(store, runtime);

        assert_eq!(lifecycle.run_due(1_000)?.probed, 1);
        lifecycle.apply_control_command(
            TargetProbeLifecycleCommand::Pause {
                target_id: "TGT-api".to_owned(),
            },
            2_000,
        );
        let paused = lifecycle.run_due(1_000_000)?;

        lifecycle.apply_control_command(
            TargetProbeLifecycleCommand::Resume {
                target_id: "TGT-api".to_owned(),
            },
            1_000_000,
        );
        let resumed = lifecycle.run_due(1_000_000)?;

        assert_eq!(paused.due, 0);
        assert_eq!(resumed.probed, 1);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[test]
    fn lifecycle_reconfigure_command_pulls_next_due_forward() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/health", "unknown")?;
        let transport = Arc::new(StaticTransport::ok(200, "ok"));
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store.clone(),
            HealthEventFanout::new_without_failure_sink(sink),
            transport,
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        );
        let mut lifecycle = TargetProbeLifecycle::new(store, runtime);

        lifecycle.run_due(1_000)?;
        lifecycle.apply_control_command(
            TargetProbeLifecycleCommand::Reconfigure {
                target_id: "TGT-api".to_owned(),
                interval_ms: 1_000,
            },
            2_000,
        );

        let schedule = lifecycle
            .schedules
            .get("TGT-api")
            .ok_or("missing target schedule")?;
        assert_eq!(schedule.interval_ms, 1_000);
        assert_eq!(schedule.next_due_millis, 3_000);
        Ok(())
    }

    #[test]
    fn in_flight_deactivation_skips_commit_after_transport_returns() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/health", "unknown")?;
        let transport = DeactivatingTransport::new(store.clone());
        let sink = Arc::new(RecordingSink::default());
        let fanout = HealthEventFanout::new_without_failure_sink(sink);

        let error = match run_target_probe_once(
            &store,
            &fanout,
            &transport,
            "TGT-api",
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        ) {
            Ok(outcome) => {
                return Err(format!("expected target to be skipped, got {outcome:?}").into());
            }
            Err(error) => error,
        };

        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert!(matches!(error, TargetProbeRuntimeError::TargetNotFound));
        Ok(())
    }

    #[test]
    fn lifecycle_rejects_zero_tick_interval() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/health", "unknown")?;
        let transport = Arc::new(StaticTransport::ok(200, "ok"));
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store.clone(),
            HealthEventFanout::new_without_failure_sink(sink),
            transport,
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        );
        let lifecycle = TargetProbeLifecycle::new(store, runtime);

        let error = match TargetProbeLifecycleWorker::spawn(
            lifecycle,
            TargetProbeLifecycleConfig {
                tick_interval: StdDuration::ZERO,
            },
        ) {
            Ok(_) => return Err("zero intervals should be rejected".into()),
            Err(error) => error,
        };

        assert_eq!(
            error,
            "target probe lifecycle tick interval must be greater than zero"
        );
        Ok(())
    }

    #[test]
    fn runtime_keeps_bounded_transition_history_for_flap_detection() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/health", "unknown")?;
        let transport = Arc::new(QueueTransport::new(vec![
            response(200, "ok"),
            response(500, "down"),
            response(200, "ok"),
            response(500, "down"),
        ]));
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store,
            HealthEventFanout::new_without_failure_sink(sink),
            transport.clone(),
            TargetProbeOptions {
                allow_private_targets: true,
                region: None,
            },
        );

        assert_eq!(runtime.run_once("TGT-api")?.state, "up");
        assert_eq!(runtime.run_once("TGT-api")?.state, "degraded");
        assert_eq!(runtime.run_once("TGT-api")?.state, "up");
        assert_eq!(runtime.run_once("TGT-api")?.state, "flapping");
        assert_eq!(transport.calls.load(Ordering::SeqCst), 4);
        Ok(())
    }

    fn response(status_code: u16, body: &str) -> ProbeHttpResponse {
        ProbeHttpResponse {
            status_code,
            body: body.to_owned(),
            tls_expires_at: None,
        }
    }

    fn seeded_store(url: &str, state: &str) -> Result<Arc<Mutex<Store>>, Box<dyn Error>> {
        seeded_store_with_headers(url, state, None)
    }

    fn seeded_store_with_headers(
        url: &str,
        state: &str,
        headers: Option<String>,
    ) -> Result<Arc<Mutex<Store>>, Box<dyn Error>> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        store.insert_target(TargetInsert {
            id: "TGT-api".to_owned(),
            url: url.to_owned(),
            name: "API".to_owned(),
            service: "api".to_owned(),
            method: "GET".to_owned(),
            headers,
            interval_ms: 60_000,
            timeout_ms: 10_000,
            expected_status: "200".to_owned(),
            body_contains: Some("ok".to_owned()),
            degraded_after: 1,
            down_after: 3,
            up_after: 1,
            active: true,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        store.commit_target_probe(canary_store::TargetProbeCommit {
            target_id: "TGT-api".to_owned(),
            state: state.to_owned(),
            consecutive_failures: 0,
            consecutive_successes: if state == "up" { 1 } else { 0 },
            check_succeeded: state == "up",
            check: canary_store::TargetCheckObservation {
                status_code: Some(200),
                latency_ms: Some(1),
                result: "success".to_owned(),
                tls_expires_at: None,
                error_detail: None,
                region: None,
            },
            now: "2026-05-28T20:00:00Z".to_owned(),
            transition: None,
        })?;
        Ok(Arc::new(Mutex::new(store)))
    }

    fn assert_parse_header_error(headers: &str, expected: &str) -> Result<(), Box<dyn Error>> {
        match parse_headers(Some(headers)) {
            Ok(parsed) => Err(format!("expected {headers} to fail, parsed {parsed:?}").into()),
            Err(error) => {
                assert_eq!(error, expected);
                Ok(())
            }
        }
    }

    fn tls_test_certificate()
    -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>, String), Box<dyn Error>> {
        let mut params = CertificateParams::new(vec!["localhost".to_owned()])?;
        params.not_after = date_time_ymd(2030, 1, 2);
        let signing_key = KeyPair::generate()?;
        let certificate = params.self_signed(&signing_key)?;
        let private_key = PrivateKeyDer::try_from(signing_key.serialize_der())
            .map_err(|_| "failed to encode TLS test private key")?;
        Ok((
            certificate.der().clone(),
            private_key,
            "2030-01-02T00:00:00Z".to_owned(),
        ))
    }

    fn spawn_tls_expiry_server(
        certificate: CertificateDer<'static>,
        private_key: PrivateKeyDer<'static>,
    ) -> Result<(SocketAddr, TlsTestServer), Box<dyn Error>> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
        let addr = listener.local_addr()?;
        let config = Arc::new(
            ServerConfig::builder_with_provider(
                rustls::crypto::aws_lc_rs::default_provider().into(),
            )
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .map_err(|error| format!("failed to choose TLS test versions: {error}"))?
            .with_no_client_auth()
            .with_single_cert(vec![certificate], private_key)
            .map_err(|error| format!("failed to build TLS test server config: {error}"))?,
        );
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
            stream
                .set_read_timeout(Some(StdDuration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            stream
                .set_write_timeout(Some(StdDuration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            let mut connection =
                ServerConnection::new(config).map_err(|error| error.to_string())?;
            while connection.is_handshaking() {
                connection
                    .complete_io(&mut stream)
                    .map_err(|error| error.to_string())?;
            }
            let mut tls = StreamOwned::new(connection, stream);
            tls.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .map_err(|error| error.to_string())
        });
        Ok((addr, handle))
    }
}
