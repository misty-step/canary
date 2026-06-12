//! Agent-native inspection helpers for the `canary` CLI.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

/// Default hosted Canary endpoint used when no endpoint is configured.
pub const DEFAULT_ENDPOINT: &str = "https://canary-obs.fly.dev";
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const WITNESS_MONITOR_NAME: &str = "canary-watchman";

/// Error returned by the CLI library.
#[derive(Debug, Error)]
pub enum CliError {
    /// Configuration or command-line input is invalid.
    #[error("{0}")]
    Message(String),
    /// Filesystem operation failed.
    #[error("filesystem error at {path}: {source}")]
    Io {
        /// Path involved in the failure.
        path: PathBuf,
        /// Underlying IO error.
        source: std::io::Error,
    },
    /// HTTP transport failed.
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// JSON parsing failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// A child command failed.
    #[error("{program} exited with status {status}: {stderr}")]
    CommandFailed {
        /// Program that was run.
        program: String,
        /// Exit status rendered for agents.
        status: String,
        /// Redacted stderr.
        stderr: String,
    },
}

/// Result alias for CLI helpers.
pub type Result<T> = std::result::Result<T, CliError>;

/// Configuration file shape.
#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    /// Base Canary endpoint.
    pub endpoint: Option<String>,
    /// Read-scoped API key.
    pub read_api_key: Option<String>,
    /// Admin-scoped API key.
    pub admin_api_key: Option<String>,
    /// Fallback API key.
    pub api_key: Option<String>,
}

/// Resolved runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Base Canary endpoint.
    pub endpoint: String,
    api_key: Option<String>,
    api_key_source: Option<String>,
}

impl Config {
    /// Resolve config from flags, environment, and a local JSON config file.
    pub fn resolve(
        endpoint_flag: Option<String>,
        key_flag: Option<String>,
        config_path: Option<PathBuf>,
    ) -> Result<Self> {
        let file_config = read_file_config(config_path)?;
        let endpoint = first_non_empty([
            endpoint_flag,
            env::var("CANARY_ENDPOINT").ok(),
            file_config.endpoint,
            Some(DEFAULT_ENDPOINT.to_owned()),
        ])
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned());

        let key_sources = [
            ("--api-key", key_flag),
            (
                "CANARY_ADMIN_API_KEY",
                env::var("CANARY_ADMIN_API_KEY").ok(),
            ),
            ("CANARY_ADMIN_KEY", env::var("CANARY_ADMIN_KEY").ok()),
            ("CANARY_READ_API_KEY", env::var("CANARY_READ_API_KEY").ok()),
            ("CANARY_READ_KEY", env::var("CANARY_READ_KEY").ok()),
            ("config.admin_api_key", file_config.admin_api_key),
            ("config.read_api_key", file_config.read_api_key),
            ("config.api_key", file_config.api_key),
            ("CANARY_API_KEY", env::var("CANARY_API_KEY").ok()),
        ];

        let mut api_key = None;
        let mut api_key_source = None;
        for (source, value) in key_sources {
            if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
                api_key = Some(value);
                api_key_source = Some(source.to_owned());
                break;
            }
        }

        Ok(Self {
            endpoint: normalize_endpoint(&endpoint),
            api_key,
            api_key_source,
        })
    }

    /// Return true when an API key is configured.
    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    /// Return a redacted API key description.
    pub fn redacted_key(&self) -> String {
        match (&self.api_key_source, &self.api_key) {
            (Some(source), Some(_key)) => format!("{source}: redacted"),
            _ => "missing".to_owned(),
        }
    }

    fn api_key(&self) -> Result<&str> {
        self.api_key.as_deref().ok_or_else(|| {
            CliError::Message(
                "missing Canary read/admin API key; set CANARY_ADMIN_API_KEY, CANARY_ADMIN_KEY, CANARY_API_KEY, CANARY_READ_API_KEY, CANARY_READ_KEY, --api-key, or config api_key".to_owned(),
            )
        })
    }
}

/// Resolve an endpoint for local-only commands without reading config files.
pub fn resolve_endpoint_without_config(endpoint_flag: Option<&str>) -> String {
    let endpoint = first_non_empty([
        endpoint_flag.map(ToOwned::to_owned),
        env::var("CANARY_ENDPOINT").ok(),
        Some(DEFAULT_ENDPOINT.to_owned()),
    ])
    .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned());
    normalize_endpoint(&endpoint)
}

/// Minimal blocking API client.
pub struct ApiClient {
    config: Config,
    client: Client,
}

impl ApiClient {
    /// Create a client for the resolved config.
    pub fn new(config: Config) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .timeout(HTTP_REQUEST_TIMEOUT)
            .build()?;
        Ok(Self { config, client })
    }

    /// Return the configured endpoint.
    pub fn endpoint(&self) -> &str {
        &self.config.endpoint
    }

    /// GET a public JSON endpoint.
    pub fn get_public_json(&self, path: &str) -> Result<Value> {
        self.get_json(path, false)
    }

    /// GET an authenticated JSON endpoint.
    pub fn get_auth_json(&self, path: &str) -> Result<Value> {
        self.get_json(path, true)
    }

    fn get_json(&self, path: &str, auth: bool) -> Result<Value> {
        let url = format!("{}{}", self.config.endpoint, path);
        let mut request = self.client.get(url);
        if auth {
            request = request.bearer_auth(self.config.api_key()?);
        }
        let response = request.send()?;
        let status = response.status();
        let body = response.text()?;
        if !status.is_success() {
            return Err(CliError::Message(format!(
                "GET {path} returned {status}: {}",
                redact_text(&body)
            )));
        }
        serde_json::from_str(&body).map_err(CliError::from)
    }
}

/// Canary query window accepted by the deployed API.
#[derive(Debug, Clone, Copy)]
pub enum Window {
    /// Last hour.
    OneHour,
    /// Last six hours.
    SixHours,
    /// Last twenty-four hours.
    TwentyFourHours,
    /// Last seven days.
    SevenDays,
    /// Last thirty days.
    ThirtyDays,
}

impl Window {
    /// Parse a query window.
    pub fn parse(input: &str) -> Result<Self> {
        match input {
            "1h" => Ok(Self::OneHour),
            "6h" => Ok(Self::SixHours),
            "24h" => Ok(Self::TwentyFourHours),
            "7d" => Ok(Self::SevenDays),
            "30d" => Ok(Self::ThirtyDays),
            _ => Err(CliError::Message(
                "invalid window; expected one of: 1h, 6h, 24h, 7d, 30d".to_owned(),
            )),
        }
    }

    /// Return the wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneHour => "1h",
            Self::SixHours => "6h",
            Self::TwentyFourHours => "24h",
            Self::SevenDays => "7d",
            Self::ThirtyDays => "30d",
        }
    }
}

/// Render mode selected by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// Stable JSON envelope.
    Json,
    /// Concise text for agent transcripts.
    Text,
}

/// Render a stable JSON command envelope.
pub fn json_envelope(command: &str, endpoint: &str, response: Value) -> Value {
    json!({
        "schema_version": 1,
        "command": command,
        "endpoint": endpoint,
        "response": response
    })
}

/// Summarize a report response.
pub fn summarize_report(value: &Value) -> Vec<String> {
    vec![
        format!("summary: {}", string_field(value, "summary")),
        format!("status: {}", string_field(value, "status")),
        format!("targets: {}", array_len(value, "targets")),
        format!("monitors: {}", array_len(value, "monitors")),
        format!("incidents: {}", array_len(value, "incidents")),
        format!("error_groups: {}", array_len(value, "error_groups")),
    ]
}

/// Summarize a status response as services.
pub fn summarize_services(value: &Value, state_filter: Option<&str>) -> Vec<String> {
    let mut lines = vec![format!("summary: {}", string_field(value, "summary"))];
    lines.extend(service_lines(value, "targets", state_filter));
    lines.extend(service_lines(value, "monitors", state_filter));
    if lines.len() == 1 {
        lines.push("services: 0".to_owned());
    }
    lines
}

/// Summarize an error query response.
pub fn summarize_query(value: &Value) -> Vec<String> {
    vec![
        format!("summary: {}", string_field(value, "summary")),
        format!("service: {}", string_field(value, "service")),
        format!("window: {}", string_field(value, "window")),
        format!("total_errors: {}", number_field(value, "total_errors")),
        format!("groups: {}", array_len(value, "groups")),
    ]
}

/// Summarize incidents.
pub fn summarize_incidents(value: &Value) -> Vec<String> {
    vec![
        format!("summary: {}", string_field(value, "summary")),
        format!("incidents: {}", array_len(value, "incidents")),
    ]
}

/// Summarize timeline events.
pub fn summarize_timeline(value: &Value) -> Vec<String> {
    vec![
        format!("summary: {}", string_field(value, "summary")),
        format!("service: {}", string_field(value, "service")),
        format!("window: {}", string_field(value, "window")),
        format!("events: {}", array_len(value, "events")),
    ]
}

/// Summarize targets.
pub fn summarize_targets(value: &Value) -> Vec<String> {
    summarize_collection(value, "targets")
}

/// Summarize monitors.
pub fn summarize_monitors(value: &Value) -> Vec<String> {
    summarize_collection(value, "monitors")
}

/// Summarize dogfood inventory.
pub fn summarize_dogfood(value: &Value) -> Vec<String> {
    let summary = value.get("summary").unwrap_or(value);
    vec![
        format!("covered: {}", number_field(summary, "covered")),
        format!("partial: {}", number_field(summary, "partial")),
        format!("blocked: {}", number_field(summary, "blocked")),
        format!("ignored: {}", number_field(summary, "ignored")),
        format!("strict_failures: {}", dogfood_strict_failure_count(value)),
    ]
}

/// Count dogfood strict-mode failures from an inventory JSON report.
pub fn dogfood_strict_failure_count(value: &Value) -> usize {
    value
        .get("strict_failures")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// Run the existing dogfood inventory command and return its JSON output.
pub fn run_dogfood_inventory(repo_root: &Path, strict: bool) -> Result<Value> {
    let program = repo_root.join("bin/dogfood-inventory");
    let mut command = Command::new(&program);
    command.current_dir(repo_root).arg("--json");
    if strict {
        command.arg("--strict");
    }
    let output = command.output().map_err(|source| CliError::Io {
        path: program.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(CliError::CommandFailed {
            program: program.display().to_string(),
            status: output
                .status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
            stderr: redact_text(&String::from_utf8_lossy(&output.stderr)),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(CliError::from)
}

/// Find the repository root by walking upward until the dogfood command exists.
pub fn find_repo_root(start: &Path) -> Result<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join("bin/dogfood-inventory").is_file() {
            return Ok(current);
        }
        if !current.pop() {
            return Err(CliError::Message(
                "could not find Canary repo root containing bin/dogfood-inventory".to_owned(),
            ));
        }
    }
}

/// Build a doctor report from live API responses.
pub fn doctor_report(client: &ApiClient, repo_root: &Path) -> Value {
    let healthz = public_probe(client, "/healthz");
    let readyz = public_probe(client, "/readyz");
    let auth_configured = client.config.has_api_key();
    let report = auth_probe(client, "/api/v1/report?window=1h");
    let status = auth_probe(client, "/api/v1/status?window=1h");
    let targets = auth_probe(client, "/api/v1/targets");
    let monitors = auth_probe(client, "/api/v1/monitors");
    let incidents = auth_probe(client, "/api/v1/incidents");
    let canary_errors = auth_probe(client, "/api/v1/query?service=canary&window=1h");
    let witness = witness_monitor_report(&status, &monitors);
    let dogfood = match run_dogfood_inventory(repo_root, false) {
        Ok(value) => json!({"ok": true, "summary": summarize_dogfood(&value), "response": value}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    };

    json!({
        "schema_version": 1,
        "endpoint": client.endpoint(),
        "key": client.config.redacted_key(),
        "key_scope": key_scope(&report, &targets),
        "auth_configured": auth_configured,
        "reachability": {
            "healthz": healthz,
            "readyz": readyz
        },
        "summary": report,
        "services": status,
        "admin": targets,
        "monitors": monitors,
        "incidents": incidents,
        "canary_errors": canary_errors,
        "witness": witness,
        "dogfood": dogfood,
        "worker_readiness": {
            "available": false,
            "reason": "backlog item #034 has not landed"
        }
    })
}

/// Render doctor text.
pub fn summarize_doctor(value: &Value) -> Vec<String> {
    let mut lines = vec![
        format!("endpoint: {}", string_field(value, "endpoint")),
        format!("key: {}", string_field(value, "key")),
        format!("key_scope: {}", string_field(value, "key_scope")),
    ];
    for key in ["healthz", "readyz"] {
        let probe = value
            .get("reachability")
            .and_then(|reachability| reachability.get(key))
            .unwrap_or(&Value::Null);
        lines.push(format!("{}: {}", key, probe_status(probe)));
    }
    lines.push(format!("summary: {}", probe_summary(value.get("summary"))));
    lines.push(format!(
        "services: {}",
        probe_summary(value.get("services"))
    ));
    lines.push(format!(
        "witness: {}",
        witness_summary(value.get("witness"))
    ));
    lines.push(format!(
        "canary_errors: {}",
        probe_summary(value.get("canary_errors"))
    ));
    lines.push(format!(
        "incidents: {}",
        probe_summary(value.get("incidents"))
    ));
    lines.push(format!("dogfood: {}", probe_summary(value.get("dogfood"))));
    lines.push("worker_readiness: unavailable until #034 lands".to_owned());
    lines
}

fn read_file_config(config_path: Option<PathBuf>) -> Result<FileConfig> {
    let Some(path) = config_path.or_else(default_config_path) else {
        return Ok(FileConfig::default());
    };
    if !path.exists() {
        return Ok(FileConfig::default());
    }
    let body = fs::read_to_string(&path).map_err(|source| CliError::Io {
        path: path.clone(),
        source,
    })?;
    serde_json::from_str(&body).map_err(CliError::from)
}

fn default_config_path() -> Option<PathBuf> {
    if let Ok(path) = env::var("CANARY_CONFIG")
        && !path.trim().is_empty()
    {
        return Some(PathBuf::from(path));
    }
    env::var("HOME")
        .ok()
        .filter(|home| !home.trim().is_empty())
        .map(|home| PathBuf::from(home).join(".config/canary/config.json"))
}

fn first_non_empty(values: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
}

fn normalize_endpoint(endpoint: &str) -> String {
    endpoint.trim().trim_end_matches('/').to_owned()
}

fn redact_text(input: &str) -> String {
    input
        .split_whitespace()
        .map(|part| {
            if part.starts_with("sk_") {
                "sk_...".to_owned()
            } else {
                part.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

fn number_field(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn array_len(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_array).map_or(0, Vec::len)
}

fn service_lines(value: &Value, key: &str, state_filter: Option<&str>) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item
                        .get("service")
                        .or_else(|| item.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let state = item_state(item);
                    if state_filter.is_some_and(|filter| filter != state) {
                        return None;
                    }
                    Some(format!("{key}: {name} {state}"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn summarize_collection(value: &Value, key: &str) -> Vec<String> {
    let mut lines = vec![format!("{key}: {}", array_len(value, key))];
    if let Some(items) = value.get(key).and_then(Value::as_array) {
        lines.extend(items.iter().take(10).map(|item| {
            let name = item
                .get("service")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let state = item
                .get("state")
                .or_else(|| item.get("last_check_in_status"))
                .and_then(Value::as_str)
                .map_or_else(|| item_state(item).to_owned(), ToOwned::to_owned);
            format!("- {name}: {state}")
        }));
    }
    lines
}

fn item_state(item: &Value) -> &str {
    if let Some(state) = item
        .get("state")
        .or_else(|| item.get("last_check_in_status"))
        .and_then(Value::as_str)
    {
        return state;
    }
    match item.get("active").and_then(Value::as_bool) {
        Some(true) => "active",
        Some(false) => "paused",
        None => "unknown",
    }
}

fn public_probe(client: &ApiClient, path: &str) -> Value {
    match client.get_public_json(path) {
        Ok(value) => json!({"ok": true, "response": value}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    }
}

fn auth_probe(client: &ApiClient, path: &str) -> Value {
    match client.get_auth_json(path) {
        Ok(value) => json!({"ok": true, "summary": summary_for_probe(&value), "response": value}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    }
}

fn key_scope(read_probe: &Value, admin_probe: &Value) -> &'static str {
    match (
        read_probe
            .get("ok")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        admin_probe
            .get("ok")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    ) {
        (true, true) => "admin",
        (true, false) => "read-only",
        (false, _) => "missing-or-invalid",
    }
}

fn summary_for_probe(value: &Value) -> Vec<String> {
    if value.get("overall").is_some() {
        summarize_services(value, None)
    } else if value.get("service").is_some() && value.get("total_errors").is_some() {
        summarize_query(value)
    } else if value.get("incidents").is_some() && value.get("summary").is_some() {
        summarize_incidents(value)
    } else {
        summarize_report(value)
    }
}

fn witness_monitor_report(status_probe: &Value, monitors_probe: &Value) -> Value {
    let status_monitor = find_witness_monitor(status_probe);
    let configured_monitor = find_witness_monitor(monitors_probe);

    if let Some(monitor) = status_monitor.filter(|monitor| monitor_has_check_in(monitor)) {
        return json!({
            "status": "observed",
            "monitor": monitor_name(monitor),
            "state": item_state(monitor),
            "last_check_in_status": string_or_unknown(monitor, "last_check_in_status"),
            "last_check_in_at": string_or_unknown(monitor, "last_check_in_at"),
            "config_seen": configured_monitor.is_some(),
            "mode": configured_monitor
                .and_then(|item| item.get("mode"))
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            "expected_every_ms": configured_monitor
                .and_then(|item| item.get("expected_every_ms"))
                .and_then(Value::as_i64)
                .unwrap_or(0)
        });
    }

    if let Some(monitor) = configured_monitor.or(status_monitor) {
        return json!({
            "status": "configured",
            "monitor": monitor_name(monitor),
            "state": "unknown",
            "mode": string_or_unknown(monitor, "mode"),
            "expected_every_ms": monitor
                .get("expected_every_ms")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            "reason": "monitor is configured but /api/v1/status did not report check-in state"
        });
    }

    let status_ok = probe_ok(status_probe);
    let monitors_ok = probe_ok(monitors_probe);
    if status_ok || monitors_ok {
        json!({
            "status": "missing",
            "monitor": WITNESS_MONITOR_NAME,
            "reason": "no canary-watchman monitor found in status or monitor configuration"
        })
    } else {
        json!({
            "status": "unavailable",
            "monitor": WITNESS_MONITOR_NAME,
            "reason": "could not inspect status or monitor configuration"
        })
    }
}

fn find_witness_monitor(probe: &Value) -> Option<&Value> {
    probe
        .get("response")
        .and_then(|response| response.get("monitors"))
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find(|item| monitor_matches(item)))
}

fn monitor_matches(item: &Value) -> bool {
    item.get("name")
        .or_else(|| item.get("service"))
        .and_then(Value::as_str)
        .is_some_and(|value| value == WITNESS_MONITOR_NAME)
}

fn monitor_name(item: &Value) -> &str {
    item.get("name")
        .or_else(|| item.get("service"))
        .and_then(Value::as_str)
        .unwrap_or(WITNESS_MONITOR_NAME)
}

fn monitor_has_check_in(item: &Value) -> bool {
    item.get("last_check_in_at")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        && item
            .get("last_check_in_status")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
}

fn string_or_unknown<'a>(item: &'a Value, key: &str) -> &'a str {
    item.get(key).and_then(Value::as_str).unwrap_or("unknown")
}

fn probe_ok(probe: &Value) -> bool {
    probe.get("ok").and_then(Value::as_bool).unwrap_or(false)
}

fn witness_summary(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "missing".to_owned();
    };
    match string_field(value, "status").as_str() {
        "observed" => format!(
            "{} {} last_check_in={} at {}",
            string_field(value, "monitor"),
            string_field(value, "state"),
            string_field(value, "last_check_in_status"),
            string_field(value, "last_check_in_at")
        ),
        "configured" => format!(
            "{} configured, status readback pending",
            string_field(value, "monitor")
        ),
        "missing" => format!("{} missing", string_field(value, "monitor")),
        "unavailable" => format!("{} unavailable", string_field(value, "monitor")),
        other => other.to_owned(),
    }
}

fn probe_status(value: &Value) -> String {
    if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        "ok".to_owned()
    } else {
        format!("error: {}", string_field(value, "error"))
    }
}

fn probe_summary(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "missing".to_owned();
    };
    if !value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return format!("error: {}", string_field(value, "error"));
    }
    value
        .get("summary")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
        .unwrap_or("ok")
        .to_owned()
}

/// URL-encode a path query component.
pub fn encode(input: &str) -> String {
    urlencoding::encode(input).into_owned()
}

/// Print lines to stdout.
pub fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

/// Print JSON to stdout.
pub fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Stable command metadata for MCP generation.
#[derive(Debug, Serialize)]
pub struct ToolSpec {
    /// Tool name.
    pub name: &'static str,
    /// Tool description.
    pub description: &'static str,
    /// JSON schema for arguments.
    pub input_schema: Value,
}

/// Return the CLI-backed tool manifest.
pub fn tool_manifest() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "canary_summary",
            description: "Inspect global Canary status for a query window.",
            input_schema: json!({"type":"object","properties":{"window":{"type":"string","enum":["1h","6h","24h","7d","30d"]}}}),
        },
        ToolSpec {
            name: "canary_errors",
            description: "Inspect recent error groups for one service.",
            input_schema: json!({"type":"object","required":["service"],"properties":{"service":{"type":"string"},"window":{"type":"string","enum":["1h","6h","24h","7d","30d"]}}}),
        },
        ToolSpec {
            name: "canary_doctor",
            description: "Run the agent-oriented Canary doctor check.",
            input_schema: json!({"type":"object","properties":{}}),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn witness_monitor_report_keeps_never_checked_in_monitor_configured() {
        let status_probe = json!({
            "ok": true,
            "response": {
                "monitors": [{
                    "name": "canary-watchman",
                    "service": "canary",
                    "state": "unknown",
                    "last_check_in_status": null,
                    "last_check_in_at": null
                }]
            }
        });
        let monitors_probe = json!({
            "ok": true,
            "response": {
                "monitors": [{
                    "name": "canary-watchman",
                    "service": "canary",
                    "mode": "ttl",
                    "expected_every_ms": 600000
                }]
            }
        });

        let report = witness_monitor_report(&status_probe, &monitors_probe);

        assert_eq!(report["status"], "configured");
        assert_eq!(report["monitor"], "canary-watchman");
        assert_eq!(report["mode"], "ttl");
    }
}
