//! Single-target probe runtime adapter.
//!
//! Scheduling stays outside this module. This file owns the concrete runtime
//! work needed to turn one target row into one persisted probe observation:
//! SSRF validation, bounded HTTP execution, Phoenix-compatible result mapping,
//! store commit, and post-commit webhook fanout.

use std::{
    collections::BTreeMap,
    io::Read,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration as StdDuration, Instant},
};

use canary_core::health::state_machine::{Counters, HealthState, Thresholds};
use canary_store::{ActiveTargetProbeSchedule, Store, TargetProbeSnapshot};
use canary_workers::health::{
    ObservationContext, TargetProbeObservation, TargetSnapshot, plan_target_probe,
};
use reqwest::{Method, Url, redirect::Policy};
use serde_json::Value;

use crate::{EventSink, current_rfc3339, current_unix_millis};

const MAX_PROBE_BODY_BYTES: u64 = 64 * 1024;

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
    /// Health transition event enqueued after commit, when any.
    pub transition_event: Option<String>,
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
    event_sink: Arc<dyn EventSink>,
    transport: Arc<dyn ProbeTransport>,
    options: TargetProbeOptions,
    transition_history: Mutex<BTreeMap<String, Vec<(HealthState, i64)>>>,
}

impl TargetProbeRuntime {
    /// Build a target probe runtime from explicit side-effect boundaries.
    pub fn new(
        store: Arc<Mutex<Store>>,
        event_sink: Arc<dyn EventSink>,
        transport: Arc<dyn ProbeTransport>,
        options: TargetProbeOptions,
    ) -> Self {
        Self {
            store,
            event_sink,
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
            self.event_sink.as_ref(),
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
}

/// Bounded lifecycle adapter for active HTTP target probes.
pub struct TargetProbeLifecycle {
    store: Arc<Mutex<Store>>,
    runtime: TargetProbeRuntime,
    schedules: BTreeMap<String, ScheduledTarget>,
}

impl TargetProbeLifecycle {
    /// Build a lifecycle adapter from the shared store and probe runtime.
    pub fn new(store: Arc<Mutex<Store>>, runtime: TargetProbeRuntime) -> Self {
        Self {
            store,
            runtime,
            schedules: BTreeMap::new(),
        }
    }

    /// Load active targets, execute due probes sequentially, and update next due times.
    pub fn run_due(&mut self, now_millis: i64) -> Result<TargetProbeLifecycleReport, String> {
        let active = self.load_active_schedules()?;
        self.reconcile(active, now_millis);

        let due_targets = self
            .schedules
            .iter()
            .filter(|(_, schedule)| schedule.next_due_millis <= now_millis)
            .map(|(target_id, _)| target_id.clone())
            .collect::<Vec<_>>();

        let mut report = TargetProbeLifecycleReport {
            loaded: self.schedules.len(),
            due: due_targets.len(),
            ..TargetProbeLifecycleReport::default()
        };

        for target_id in due_targets {
            match self.runtime.run_once(&target_id) {
                Ok(_) => report.probed += 1,
                Err(TargetProbeRuntimeError::TargetNotFound) => report.skipped_missing += 1,
                Err(_) => report.failed += 1,
            }

            if let Some(schedule) = self.schedules.get_mut(&target_id) {
                schedule.next_due_millis =
                    next_due_millis(&target_id, schedule.interval_ms, now_millis);
            }
        }

        Ok(report)
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

    fn reconcile(&mut self, active: Vec<ActiveTargetProbeSchedule>, now_millis: i64) {
        let mut next = BTreeMap::new();
        for target in active {
            let interval_ms = target.interval_ms.max(1_000);
            let next_due_millis = self
                .schedules
                .remove(&target.target_id)
                .filter(|existing| existing.interval_ms == interval_ms)
                .map(|existing| existing.next_due_millis)
                .unwrap_or(now_millis);
            next.insert(
                target.target_id,
                ScheduledTarget {
                    interval_ms,
                    next_due_millis,
                },
            );
        }
        self.schedules = next;
    }
}

/// Dedicated OS-thread runner for target probe lifecycle passes.
pub struct TargetProbeLifecycleWorker {
    control: Arc<LifecycleControl>,
    handle: Option<JoinHandle<()>>,
}

impl TargetProbeLifecycleWorker {
    /// Spawn one named background thread that probes due active targets sequentially.
    pub fn spawn(
        lifecycle: TargetProbeLifecycle,
        config: TargetProbeLifecycleConfig,
    ) -> Result<Self, String> {
        if config.tick_interval.is_zero() {
            return Err(
                "target probe lifecycle tick interval must be greater than zero".to_owned(),
            );
        }

        let control = Arc::new(LifecycleControl::default());
        let thread_control = control.clone();
        let handle = thread::Builder::new()
            .name("canary-target-probes".to_owned())
            .spawn(move || run_lifecycle_worker(lifecycle, config.tick_interval, thread_control))
            .map_err(|error| format!("failed to spawn target probe worker: {error}"))?;

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

    /// Request shutdown without waiting for an in-flight probe to finish.
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
            Err(_) => Err("target probe worker panicked".to_owned()),
        }
    }
}

impl Drop for TargetProbeLifecycleWorker {
    fn drop(&mut self) {
        self.stop();
        let _ = self.join_handle();
    }
}

/// Execute and persist exactly one target probe.
pub fn run_target_probe_once(
    store: &Arc<Mutex<Store>>,
    event_sink: &dyn EventSink,
    transport: &dyn ProbeTransport,
    target_id: &str,
    options: TargetProbeOptions,
) -> Result<TargetProbeOutcome, TargetProbeRuntimeError> {
    Ok(run_target_probe_once_with_history(
        store,
        event_sink,
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
    event_sink: &dyn EventSink,
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
    let commit = {
        let mut store = store
            .lock()
            .map_err(|_| TargetProbeRuntimeError::StoreLock)?;
        store.commit_target_probe(plan.commit)?
    };
    let transition_event = commit.transition.as_ref().map(|transition| {
        let _ = event_sink.enqueue_event(&transition.event, &transition.payload_json);
        if let Some(event) = &transition.incident_event {
            let _ = event_sink.enqueue_event(&event.event, &event.payload_json);
        }
        transition.event.clone()
    });

    let outcome = TargetProbeOutcome {
        target_id: response_target_id,
        result: response_result,
        state: response_state,
        sequence: commit.sequence,
        transition_event,
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
                lifecycle.run_due(current_unix_millis())
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

    match transport.probe(request) {
        Ok(response) => response_observation(
            response,
            &snapshot.expected_status,
            snapshot.body_contains.as_deref(),
            Some(elapsed_millis(started)),
            options.region.clone(),
        ),
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
    let mut parsed = BTreeMap::new();
    for (name, value) in object {
        let Some(value) = value.as_str() else {
            return Err(format!("target header {name} must be a string"));
        };
        parsed.insert(name.clone(), value.to_owned());
    }
    Ok(parsed)
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
    match value {
        "unknown" => Ok(HealthState::Unknown),
        "up" => Ok(HealthState::Up),
        "degraded" => Ok(HealthState::Degraded),
        "down" => Ok(HealthState::Down),
        "paused" => Ok(HealthState::Paused),
        "flapping" => Ok(HealthState::Flapping),
        _ => Err(TargetProbeRuntimeError::InvalidTarget(format!(
            "unknown health state: {value}"
        ))),
    }
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
        sync::{
            Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use canary_store::{TargetInsert, WebhookSubscriptionInsert};

    use super::*;

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
    fn ssrf_block_persists_failed_probe_without_opening_transport() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/health", "up")?;
        let transport = StaticTransport::ok(200, "ok");
        let sink = RecordingSink::default();

        let outcome = run_target_probe_once(
            &store,
            &sink,
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
        let sink = RecordingSink::default();

        let outcome = run_target_probe_once(
            &store,
            &sink,
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
    fn lifecycle_loads_active_targets_and_runs_due_probes_sequentially()
    -> Result<(), Box<dyn Error>> {
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
            sink,
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
            }
        );
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
            sink,
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
    fn lifecycle_rejects_zero_tick_interval() -> Result<(), Box<dyn Error>> {
        let store = seeded_store("http://127.0.0.1/health", "unknown")?;
        let transport = Arc::new(StaticTransport::ok(200, "ok"));
        let sink = Arc::new(RecordingSink::default());
        let runtime = TargetProbeRuntime::new(
            store.clone(),
            sink,
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
            sink,
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
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        store.insert_target(TargetInsert {
            id: "TGT-api".to_owned(),
            url: url.to_owned(),
            name: "API".to_owned(),
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
}
