//! Agent-native inspection helpers for the `canary` CLI.

use std::{
    collections::{BTreeSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Default hosted Canary endpoint used when no endpoint is configured.
pub const DEFAULT_ENDPOINT: &str = "https://canary-obs.fly.dev";
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const WITNESS_MONITOR_NAME: &str = "canary-watchman";
const WITNESS_WORKFLOW_NAME: &str = "Canary Witness";
const WITNESS_WORKFLOW_BRANCH: &str = "master";
const WITNESS_ARTIFACT_PATTERN: &str = "canary-witness-<run_id>";
const WITNESS_RUN_LIST_COMMAND: &str = "gh run list --workflow \"Canary Witness\" --branch master --limit 3 --json databaseId,status,conclusion,createdAt,updatedAt,url,event,workflowName";
const DEFAULT_INTEGRATION_ENDPOINT_ENV: &str = "CANARY_ENDPOINT";
const DEFAULT_INTEGRATION_SERVER_KEY_ENV: &str = "CANARY_API_KEY";
const DEFAULT_INTEGRATION_PUBLIC_ENDPOINT_ENV: &str = "NEXT_PUBLIC_CANARY_ENDPOINT";
const DEFAULT_INTEGRATION_PUBLIC_KEY_ENV: &str = "NEXT_PUBLIC_CANARY_API_KEY";
const DOGFOOD_VALUE_ERROR_GROUP_SUBJECT_LIMIT: usize = 3;

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
    /// Ingest-scoped API key.
    pub ingest_api_key: Option<String>,
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
        Self::resolve_with_mode(endpoint_flag, key_flag, config_path, KeyMode::Read)
    }

    /// Resolve config from flags, environment, and local JSON for ingest/write commands.
    pub fn resolve_for_ingest(
        endpoint_flag: Option<String>,
        key_flag: Option<String>,
        config_path: Option<PathBuf>,
    ) -> Result<Self> {
        Self::resolve_with_mode(endpoint_flag, key_flag, config_path, KeyMode::Ingest)
    }

    fn resolve_with_mode(
        endpoint_flag: Option<String>,
        key_flag: Option<String>,
        config_path: Option<PathBuf>,
        mode: KeyMode,
    ) -> Result<Self> {
        let file_config = read_file_config(config_path)?;
        let endpoint = first_non_empty([
            endpoint_flag,
            env::var("CANARY_ENDPOINT").ok(),
            file_config.endpoint,
            Some(DEFAULT_ENDPOINT.to_owned()),
        ])
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned());

        let key_sources = match mode {
            KeyMode::Read => vec![
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
            ],
            KeyMode::Ingest => vec![
                ("--api-key", key_flag),
                (
                    "CANARY_INGEST_API_KEY",
                    env::var("CANARY_INGEST_API_KEY").ok(),
                ),
                ("CANARY_INGEST_KEY", env::var("CANARY_INGEST_KEY").ok()),
                (
                    "CANARY_ADMIN_API_KEY",
                    env::var("CANARY_ADMIN_API_KEY").ok(),
                ),
                ("CANARY_ADMIN_KEY", env::var("CANARY_ADMIN_KEY").ok()),
                ("config.ingest_api_key", file_config.ingest_api_key),
                ("config.admin_api_key", file_config.admin_api_key),
                ("config.api_key", file_config.api_key),
                ("CANARY_API_KEY", env::var("CANARY_API_KEY").ok()),
            ],
        };

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
                "missing Canary API key; set --api-key, CANARY_ADMIN_API_KEY, CANARY_ADMIN_KEY, CANARY_INGEST_API_KEY, CANARY_INGEST_KEY, CANARY_READ_API_KEY, CANARY_READ_KEY, CANARY_API_KEY, or config api_key".to_owned(),
            )
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum KeyMode {
    Read,
    Ingest,
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

    /// POST an authenticated JSON request and parse the JSON response.
    pub fn post_auth_json(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{}", self.config.endpoint, path);
        let response = self
            .client
            .post(url)
            .bearer_auth(self.config.api_key()?)
            .json(body)
            .send()?;
        let status = response.status();
        let body = response.text()?;
        if !status.is_success() {
            return Err(CliError::Message(format!(
                "POST {path} returned {status}: {}",
                redact_text(&body)
            )));
        }
        serde_json::from_str(&body).map_err(CliError::from)
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

/// Input shared by integration discovery, planning, and patching.
#[derive(Debug, Clone)]
pub struct IntegrationInput {
    /// Local project path or remote platform project name.
    pub target: PathBuf,
    /// Optional service override.
    pub service: Option<String>,
    /// Optional production URL override.
    pub production_url: Option<String>,
    /// Optional platform project override.
    pub platform_project: Option<String>,
    /// Canary endpoint used in generated snippets.
    pub endpoint: String,
}

/// Request for enrolling one service through Canary.
#[derive(Debug, Clone)]
pub struct IntegrationEnrollRequest {
    /// Service name.
    pub service: String,
    /// Health URL to enroll as an HTTP target.
    pub url: String,
    /// Runtime environment label.
    pub environment: String,
    /// Target probe interval in milliseconds.
    pub interval_ms: Option<i64>,
    /// Redact returned secret material.
    pub redact: bool,
    /// Optional local project root whose receipt should be updated after enrollment.
    pub receipt_root: Option<PathBuf>,
}

/// Local integration receipt filename.
const INTEGRATION_RECEIPT_PATH: &str = ".canary/integration.json";

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

/// Summarize remediation claim responses.
pub fn summarize_claims(value: &Value) -> Vec<String> {
    if value.get("claims").is_some() {
        let current = value
            .get("current_claim")
            .and_then(|claim| claim.get("owner"))
            .and_then(Value::as_str)
            .unwrap_or("none");
        vec![
            format!("summary: {}", string_field(value, "summary")),
            format!("claims: {}", array_len(value, "claims")),
            format!("limit: {}", number_field(value, "limit")),
            format!("current_owner: {current}"),
            format!("truncated: {}", bool_field(value, "truncated")),
            format!("cursor: {}", nullable_string_field(value, "cursor")),
        ]
    } else {
        vec![
            format!("id: {}", string_field(value, "id")),
            format!(
                "subject: {} {}",
                string_field(value, "subject_type"),
                string_field(value, "subject_id")
            ),
            format!("owner: {}", string_field(value, "owner")),
            format!("state: {}", string_field(value, "state")),
        ]
    }
}

/// Summarize a telemetry event write receipt.
pub fn summarize_event(value: &Value) -> Vec<String> {
    vec![
        format!("id: {}", string_field(value, "id")),
        format!("service: {}", string_field(value, "service")),
        format!("event: {}", string_field(value, "event")),
        format!("name: {}", string_field(value, "name")),
        format!("severity: {}", string_field(value, "severity")),
        format!(
            "retention_class: {}",
            string_field(value, "retention_class")
        ),
        format!("privacy_policy: {}", string_field(value, "privacy_policy")),
        format!(
            "sampling_policy: {}",
            string_field(value, "sampling_policy")
        ),
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

/// Summarize a dogfood value receipt or aggregate value summary.
pub fn summarize_dogfood_value(value: &Value) -> Vec<String> {
    if value.get("service").is_some() {
        return vec![
            format!("service: {}", string_field(value, "service")),
            format!("value_state: {}", string_field(value, "value_state")),
            format!(
                "coverage: {}",
                value
                    .pointer("/coverage/verdict")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ),
            format!(
                "health: {}",
                value
                    .pointer("/health/state")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ),
            format!(
                "errors: {}",
                value
                    .pointer("/error_counts/total")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
            format!(
                "incidents_open: {}",
                value
                    .pointer("/incident_counts/open")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
            format!(
                "verification: {}",
                value
                    .pointer("/verification/status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ),
            format!("next_action: {}", string_field(value, "next_action")),
        ];
    }

    let summary = value.get("response").unwrap_or(value);
    vec![format!(
        "covered: {}, stale: {}, blocked: {}, partial: {}, value_unproven: {}",
        number_field(summary, "covered"),
        number_field(summary, "stale"),
        number_field(summary, "blocked"),
        number_field(summary, "partial"),
        number_field(summary, "value_unproven")
    )]
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

/// Inputs for the pure dogfood value receipt builder.
#[derive(Debug)]
pub struct DogfoodValueInput<'a> {
    /// Service name to explain.
    pub service: &'a str,
    /// Query window used for live readback.
    pub window: &'a str,
    /// Dogfood inventory probe, usually `{"ok": true, "response": ...}`.
    pub dogfood: &'a Value,
    /// Live target probe.
    pub targets: &'a Value,
    /// Live monitor probe.
    pub monitors: &'a Value,
    /// Live synthesized status probe.
    pub status: &'a Value,
    /// Live error query probe for the service.
    pub query: &'a Value,
    /// Live incident list probe.
    pub incidents: &'a Value,
    /// Live timeline probe for the service.
    pub timeline: &'a Value,
    /// Claims probes keyed by discovered subject.
    pub claims: &'a Value,
    /// Annotation probes keyed by discovered subject.
    pub annotations: &'a Value,
}

/// Discover local project integration state without reading secret values.
pub fn integration_discover(input: &IntegrationInput) -> Result<Value> {
    let target = &input.target;
    let path_exists = target.exists();
    let project_root = if path_exists {
        target.canonicalize().map_err(|source| CliError::Io {
            path: target.clone(),
            source,
        })?
    } else {
        target.clone()
    };
    let project_name = input
        .platform_project
        .clone()
        .or_else(|| vercel_project_name(&project_root))
        .or_else(|| file_name_string(target))
        .unwrap_or_else(|| target.display().to_string());
    let service = input
        .service
        .clone()
        .unwrap_or_else(|| service_name_from_project(&project_name));
    let package_json = path_exists
        .then(|| project_root.join("package.json"))
        .filter(|path| path.is_file());
    let package = package_json
        .as_ref()
        .and_then(|path| read_json_file(path).ok());
    let package_manager = detect_package_manager(&project_root);
    let framework = detect_framework(&project_root, package.as_ref());
    let platform = detect_platform(&project_root);
    let platform_env = discover_platform_env(&project_root, platform);
    let local_env_names = discover_env_names(&project_root)?;
    let code_paths = discover_code_paths(&project_root)?;
    let health_routes = discover_health_routes(&project_root);
    let receipt = integration_receipt(&project_root).ok();
    let receipt_env_names = receipt
        .as_ref()
        .and_then(|value| value.get("env_names"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|name| valid_env_name(name))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let platform_env_names = platform_env
        .get("env_names")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let declared_env_names = merge_env_names(&local_env_names, &receipt_env_names);
    let env_names = merge_env_names(&declared_env_names, &platform_env_names);
    let canary_present = code_paths
        .iter()
        .any(|path| path.get("kind").and_then(Value::as_str) == Some("canary"));
    let sentry_present = code_paths
        .iter()
        .any(|path| path.get("kind").and_then(Value::as_str) == Some("sentry"));
    let canary_env_names = env_names
        .iter()
        .filter(|name| name.contains("CANARY"))
        .cloned()
        .collect::<Vec<_>>();
    let sentry_env_names = env_names
        .iter()
        .filter(|name| name.contains("SENTRY"))
        .cloned()
        .collect::<Vec<_>>();

    Ok(json!({
        "schema_version": 1,
        "target": target.display().to_string(),
        "path_exists": path_exists,
        "project_root": if path_exists { project_root.display().to_string() } else { "unresolved".to_owned() },
        "service": service,
        "platform_project": project_name,
        "framework": framework,
        "platform": platform,
        "package_manager": package_manager,
        "production_url": input.production_url,
        "health_route": health_routes.first().cloned(),
        "health_routes": health_routes,
        "integration_receipt": receipt,
        "signals": {
            "package_json": package_json.map(|path| path.display().to_string()),
            "canary_sdk_dependency": package_has_dependency(package.as_ref(), "@canary-obs/sdk"),
            "sentry_dependency": package_has_dependency(package.as_ref(), "@sentry/nextjs")
                || package_has_dependency(package.as_ref(), "@sentry/browser")
                || package_has_dependency(package.as_ref(), "@sentry/react"),
            "canary_code_paths": code_paths.iter().filter(|path| path.get("kind").and_then(Value::as_str) == Some("canary")).cloned().collect::<Vec<_>>(),
            "sentry_code_paths": code_paths.iter().filter(|path| path.get("kind").and_then(Value::as_str) == Some("sentry")).cloned().collect::<Vec<_>>(),
            "canary_present": canary_present,
            "sentry_present": sentry_present,
            "env_names": env_names.clone(),
            "local_env_names": local_env_names,
            "receipt_env_names": receipt_env_names,
            "platform_env": platform_env,
            "declared_env_names": declared_env_names,
            "platform_env_names": platform_env_names,
            "canary_env_names": canary_env_names,
            "sentry_env_names": sentry_env_names,
            "csp_mentions_connect_src": file_contains(&project_root.join("next.config.js"), "connect-src")
                || file_contains(&project_root.join("next.config.mjs"), "connect-src")
                || file_contains(&project_root.join("next.config.ts"), "connect-src")
        }
    }))
}

/// Build a reviewable integration plan from discovery.
pub fn integration_plan(input: &IntegrationInput) -> Result<Value> {
    let discovery = integration_discover(input)?;
    let target = &input.target;
    let path_exists = discovery
        .get("path_exists")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let framework = discovery
        .get("framework")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let service = discovery
        .get("service")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let production_url = input.production_url.as_deref();
    let coverage_mode = coverage_mode(framework, discovery.get("health_routes"));
    let health_url = planned_health_url(production_url, coverage_mode);
    let canary_dep = discovery
        .pointer("/signals/canary_sdk_dependency")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let canary_code_paths = discovery
        .pointer("/signals/canary_code_paths")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let server_instrumentation_present = canary_code_paths.iter().any(|path| {
        path.get("path")
            .and_then(Value::as_str)
            .is_some_and(|path| path == "instrumentation.ts")
    });
    let global_error_capture_present = canary_code_paths.iter().any(|path| {
        path.get("path")
            .and_then(Value::as_str)
            .is_some_and(is_next_error_boundary_path)
    });
    let has_health = discovery
        .get("health_routes")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty());
    let env_names = discovery
        .pointer("/signals/env_names")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let env_set = env_names
        .iter()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let mut actions = Vec::new();

    actions.push(plan_action(
        "sdk_dependency",
        if canary_dep { "present" } else { "needed" },
        "@canary-obs/sdk dependency",
        "patch",
    ));
    actions.push(plan_action(
        "server_instrumentation",
        if server_instrumentation_present {
            "present"
        } else {
            "needed"
        },
        "Next.js instrumentation.ts initializes Canary and exports onRequestError",
        "patch",
    ));
    actions.push(plan_action(
        "health_route",
        if has_health { "present" } else { "needed" },
        "GET /api/health returns a stable JSON health response",
        "patch",
    ));
    actions.push(plan_action(
        "global_error_capture",
        if global_error_capture_present {
            "present"
        } else {
            "needed"
        },
        "Next.js global error capture sends browser-visible errors to Canary",
        "patch",
    ));
    actions.push(json!({
        "kind": "env_names",
        "status": required_env_names_present(&env_set),
        "description": "Deployment should define Canary endpoint/key env names without exposing values.",
        "executor": "platform",
        "names": [
            DEFAULT_INTEGRATION_ENDPOINT_ENV,
            DEFAULT_INTEGRATION_SERVER_KEY_ENV,
            DEFAULT_INTEGRATION_PUBLIC_ENDPOINT_ENV,
            DEFAULT_INTEGRATION_PUBLIC_KEY_ENV
        ]
    }));
    actions.push(json!({
        "kind": "target_enrollment",
        "status": if health_url.is_some() { "needed" } else { "blocked" },
        "description": "Create a Canary HTTP target and ingest key through service onboarding.",
        "executor": "enroll",
        "service": service,
        "health_url": health_url,
        "blocked_reason": if health_url.is_some() { Value::Null } else { json!("pass --production-url to compute the health URL") }
    }));
    let receipt_present = receipt_is_present(discovery.get("integration_receipt"));

    actions.push(json!({
        "kind": "integration_receipt",
        "status": if receipt_present { "present" } else { "needed" },
        "description": "Write .canary/integration.json so future agents can reconcile local, platform, and live Canary state.",
        "executor": "receipt"
    }));
    actions.push(json!({
        "kind": "webhook_subscription",
        "status": "manual",
        "description": "Optional responder webhook; keep repo mutation outside Canary.",
        "executor": "admin-api",
        "events": ["error.new_class", "incident.opened", "health_check.failed"]
    }));
    actions.push(json!({
        "kind": "non_http_monitor_templates",
        "status": "available",
        "description": "Cron, worker, desktop, and CLI check-in monitor templates are ready for non-HTTP runtimes.",
        "executor": "agent"
    }));

    Ok(json!({
        "schema_version": 1,
        "target": target.display().to_string(),
        "service": service,
        "framework": framework,
        "coverage_mode": coverage_mode,
        "can_patch": path_exists && framework == "nextjs",
        "discovery": discovery,
        "static_site": static_site_artifacts(service, production_url, coverage_mode),
        "monitor_templates": monitor_templates(service, &input.endpoint),
        "actions": actions,
        "commands": {
            "patch": format!("bin/canary integrate patch {} --service {} --endpoint {}", shell_arg(&target.display().to_string()), shell_arg(service), shell_arg(&input.endpoint)),
            "enroll": health_url.map(|url| format!("bin/canary integrate enroll --service {} --url {}", shell_arg(service), shell_arg(&url))),
            "verify": format!("bin/canary errors {} --window 1h", shell_arg(service))
        }
    }))
}

/// Apply safe Next.js integration patches and report every file touched or skipped.
pub fn integration_patch(input: &IntegrationInput) -> Result<Value> {
    let plan = integration_plan(input)?;
    if !plan
        .get("can_patch")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(CliError::Message(
            "patch currently supports existing Next.js project paths only".to_owned(),
        ));
    }

    let root = input.target.canonicalize().map_err(|source| CliError::Io {
        path: input.target.clone(),
        source,
    })?;
    let service = plan
        .get("service")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut changes = Vec::new();

    changes.push(update_package_dependency(&root)?);
    changes.push(write_if_absent_or_canary(
        &instrumentation_path(&root),
        &instrumentation_source(service, &input.endpoint),
    )?);
    changes.push(write_if_absent_or_canary(
        &health_route_path(&root),
        health_route_source(),
    )?);
    changes.push(write_if_absent_or_canary(
        &global_error_path(&root),
        &global_error_source(service),
    )?);
    changes.push(write_integration_receipt(
        &root,
        &patch_integration_receipt_value(&root, input, &plan),
    )?);

    Ok(json!({
        "schema_version": 1,
        "project_root": root.display().to_string(),
        "service": service,
        "changes": changes,
        "next_steps": [
            "Review the patch before deploying.",
            "Set Canary env names in the deployment platform without committing secret values.",
            "Deploy, then run canary integrate enroll and canary errors <service> --window 1h."
        ]
    }))
}

/// Enroll a service through the hosted Canary admin API and redact secret output by default.
pub fn integration_enroll(client: &ApiClient, request: &IntegrationEnrollRequest) -> Result<Value> {
    let mut body = json!({
        "service": request.service,
        "url": request.url,
        "environment": request.environment
    });
    if let Some(interval_ms) = request.interval_ms
        && let Some(object) = body.as_object_mut()
    {
        object.insert("interval_ms".to_owned(), json!(interval_ms));
    }
    let response = client.post_auth_json("/api/v1/service-onboarding", &body)?;
    if let Some(root) = &request.receipt_root {
        write_integration_receipt(
            root,
            &enrollment_receipt_value(client.endpoint(), request, &response),
        )?;
    }
    if request.redact {
        Ok(redact_secret_value(response))
    } else {
        Ok(response)
    }
}

/// Merge local scan, receipt, live Canary, and dogfood evidence into one coverage verdict.
pub fn integration_status(
    client: &ApiClient,
    input: &IntegrationInput,
    repo_root: &Path,
) -> Result<Value> {
    let discovery = integration_discover(input)?;
    let plan = integration_plan(input)?;
    let service = discovery
        .get("service")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let targets = live_probe(|| client.get_auth_json("/api/v1/targets"));
    let monitors = live_probe(|| client.get_auth_json("/api/v1/monitors"));
    let webhooks = live_probe(|| client.get_auth_json("/api/v1/webhooks"));
    let query = live_probe(|| {
        client.get_auth_json(&format!(
            "/api/v1/query?service={}&window=1h",
            encode(service)
        ))
    });
    let dogfood = run_dogfood_inventory(repo_root, false)
        .map(|response| json!({"ok": true, "response": response}))
        .unwrap_or_else(|error| json!({"ok": false, "error": redact_text(&error.to_string())}));
    let receipt = discovery
        .get("integration_receipt")
        .cloned()
        .unwrap_or(Value::Null);
    let target = live_collection_service_match(&targets, "targets", service);
    let monitor = live_collection_service_match(&monitors, "monitors", service);
    let verdict = integration_coverage_verdict(
        service, &discovery, &targets, &monitors, &webhooks, &query, &dogfood,
    );
    let plan = reconcile_plan_with_live_state(&plan, target.as_ref(), monitor.as_ref());

    Ok(json!({
        "schema_version": 1,
        "service": service,
        "framework": discovery.get("framework").cloned().unwrap_or_else(|| json!("unknown")),
        "coverage": verdict,
        "discovery": discovery,
        "plan": plan,
        "receipt": receipt,
        "live": {
            "targets": targets,
            "monitors": monitors,
            "webhooks": webhooks,
            "query": query
        },
        "dogfood": dogfood
    }))
}

fn live_probe(fetch: impl FnOnce() -> Result<Value>) -> Value {
    match fetch() {
        Ok(response) => json!({"ok": true, "response": response}),
        Err(error) => json!({"ok": false, "error": redact_text(&error.to_string())}),
    }
}

/// Collect live evidence and build one dogfood value receipt.
pub fn dogfood_value_report(
    client: &ApiClient,
    repo_root: &Path,
    service: &str,
    window: Window,
) -> Result<Value> {
    let dogfood = json!({"ok": true, "response": run_dogfood_inventory(repo_root, false)?});
    let targets = live_probe(|| client.get_auth_json("/api/v1/targets"));
    let monitors = live_probe(|| client.get_auth_json("/api/v1/monitors"));
    let status =
        live_probe(|| client.get_auth_json(&format!("/api/v1/status?window={}", window.as_str())));
    let query = live_probe(|| {
        client.get_auth_json(&format!(
            "/api/v1/query?service={}&window={}",
            encode(service),
            window.as_str()
        ))
    });
    let incidents = live_probe(|| client.get_auth_json("/api/v1/incidents"));
    let timeline = live_probe(|| {
        client.get_auth_json(&format!(
            "/api/v1/timeline?service={}&window={}&event_type=telemetry.event&limit=10",
            encode(service),
            window.as_str()
        ))
    });
    let subjects = dogfood_value_subjects(service, &targets, &monitors, &query, &incidents);
    let claims = dogfood_subject_probes(client, &subjects, "claims");
    let annotations = dogfood_subject_probes(client, &subjects, "annotations");

    Ok(dogfood_value_receipt(&DogfoodValueInput {
        service,
        window: window.as_str(),
        dogfood: &dogfood,
        targets: &targets,
        monitors: &monitors,
        status: &status,
        query: &query,
        incidents: &incidents,
        timeline: &timeline,
        claims: &claims,
        annotations: &annotations,
    }))
}

fn dogfood_subject_probes(
    client: &ApiClient,
    subjects: &[DogfoodValueSubject],
    kind: &str,
) -> Value {
    Value::Array(
        subjects
            .iter()
            .map(|subject| {
                let path = format!(
                    "/api/v1/{kind}?subject_type={}&subject_id={}&limit=5",
                    encode(&subject.subject_type),
                    encode(&subject.subject_id)
                );
                json!({
                    "subject_type": subject.subject_type,
                    "subject_id": subject.subject_id,
                    "probe": live_probe(|| client.get_auth_json(&path))
                })
            })
            .collect(),
    )
}

/// Build a machine-readable value receipt for one dogfooded service.
pub fn dogfood_value_receipt(input: &DogfoodValueInput<'_>) -> Value {
    let surface = dogfood_service_entry(input.dogfood, input.service).unwrap_or_else(|| json!({}));
    let target = live_collection_service_match(input.targets, "targets", input.service);
    let monitor = live_collection_service_match(input.monitors, "monitors", input.service);
    let status_target = target
        .as_ref()
        .and_then(|target| dogfood_status_target_match(input.status, target));
    let status_monitor =
        dogfood_status_monitor_match(input.status, input.service, monitor.as_ref());
    let health_state = dogfood_health_state(
        input.status,
        status_target.as_ref(),
        status_monitor.as_ref(),
        target.as_ref(),
        monitor.as_ref(),
    );
    let total_errors = input
        .query
        .pointer("/response/total_errors")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let error_group_count = input
        .query
        .pointer("/response/groups")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let service_incidents = dogfood_service_incidents(input.incidents, input.service);
    let open_incidents = service_incidents
        .iter()
        .filter(|incident| dogfood_incident_is_open(incident))
        .cloned()
        .collect::<Vec<_>>();
    let recent_annotations = dogfood_recent_annotations(input.annotations);
    let recent_telemetry_events = dogfood_recent_telemetry_events(input.timeline);
    let active_claim = dogfood_active_claim(input.claims, input.annotations);
    let coverage = dogfood_coverage_verdict(&surface, input.service, input.dogfood);
    let registry_state = surface
        .get("registry_state")
        .or_else(|| surface.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let stale_current_work = dogfood_stale_current_work(&surface, total_errors, input.query);
    let query_ok = input
        .query
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let health_ok = health_state == "up";
    let verification_status =
        dogfood_value_verification_status(&coverage, health_ok, query_ok, stale_current_work);
    let value_state = dogfood_value_state(&coverage, health_ok, query_ok, stale_current_work);
    let next_action = dogfood_value_next_action(
        &surface,
        stale_current_work,
        &health_state,
        total_errors,
        input.window,
    );

    json!({
        "schema_version": 1,
        "service": input.service,
        "window": input.window,
        "value_state": value_state,
        "coverage": {
            "verdict": coverage,
            "registry_state": registry_state,
            "evidence_stale": bool_field(&surface, "evidence_stale"),
            "receipt_seen": surface.get("receipt_seen").cloned().unwrap_or(Value::Null),
            "receipt_stale": surface.get("receipt_stale").cloned().unwrap_or(Value::Null),
            "completed_ticket_next_action": bool_field(&surface, "completed_ticket_next_action"),
            "reasons": surface.get("reasons").cloned().unwrap_or_else(|| json!([]))
        },
        "registry": {
            "state": registry_state,
            "owner": surface.get("owner").cloned().unwrap_or(Value::Null),
            "platform": surface.get("platform").cloned().unwrap_or(Value::Null),
            "production_url": surface.get("production_url").cloned().unwrap_or(Value::Null),
            "health_url": surface.get("health_url").cloned().unwrap_or(Value::Null),
            "last_checked_at": surface.get("last_checked_at").cloned().unwrap_or(Value::Null),
            "failure_mode": surface.get("failure_mode").cloned().unwrap_or(Value::Null),
            "next_action": surface.get("next_action").cloned().unwrap_or(Value::Null)
        },
        "health": {
            "state": health_state,
            "target": target.unwrap_or(Value::Null),
            "monitor": monitor.unwrap_or(Value::Null),
            "status_target": status_target.unwrap_or(Value::Null),
            "status_monitor": status_monitor.unwrap_or(Value::Null)
        },
        "error_counts": {
            "ok": query_ok,
            "total": total_errors,
            "groups": error_group_count,
            "window": input.window
        },
        "incident_counts": {
            "total": service_incidents.len(),
            "open": open_incidents.len()
        },
        "open_incidents": open_incidents,
        "active_remediation_claim": active_claim,
        "recent_annotations": recent_annotations,
        "recent_telemetry_events": recent_telemetry_events,
        "verification": {
            "kind": "synthetic",
            "status": verification_status,
            "last_verified_outcome": dogfood_last_verified_outcome(input.service, input.window, &coverage, &health_state, query_ok, total_errors, stale_current_work)
        },
        "next_action": next_action,
        "evidence": {
            "dogfood": input.dogfood,
            "targets": input.targets,
            "monitors": input.monitors,
            "status": input.status,
            "query": input.query,
            "incidents": input.incidents,
            "timeline": input.timeline,
            "claims": input.claims,
            "annotations": input.annotations
        }
    })
}

/// Summarize value-receipt coverage from a dogfood inventory.
pub fn dogfood_value_summary(value: &Value) -> Value {
    let inventory = value.get("response").unwrap_or(value);
    let summary = inventory.get("summary").unwrap_or(inventory);
    let surfaces = inventory
        .get("surfaces")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let stale = surfaces
        .iter()
        .filter(|surface| {
            bool_field(surface, "evidence_stale")
                || bool_field(surface, "completed_ticket_next_action")
                || surface
                    .get("receipt_stale")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .count();
    let value_unproven = surfaces
        .iter()
        .filter(|surface| {
            string_field(surface, "coverage") != "ignored"
                && !surface
                    .get("receipt_seen")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .count();

    json!({
        "covered": number_field(summary, "covered"),
        "stale": stale,
        "blocked": number_field(summary, "blocked"),
        "partial": number_field(summary, "partial"),
        "ignored": number_field(summary, "ignored"),
        "value_unproven": value_unproven,
        "strict_failures": dogfood_strict_failure_count(inventory)
    })
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct DogfoodValueSubject {
    subject_type: String,
    subject_id: String,
}

fn dogfood_value_subjects(
    service: &str,
    targets: &Value,
    monitors: &Value,
    query: &Value,
    incidents: &Value,
) -> Vec<DogfoodValueSubject> {
    let mut subjects = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(id) = live_collection_service_match(targets, "targets", service)
        .and_then(|target| target.get("id").and_then(Value::as_str).map(str::to_owned))
    {
        push_dogfood_subject(&mut subjects, &mut seen, "target", &id);
    }
    if let Some(monitor) = live_collection_service_match(monitors, "monitors", service) {
        let id = monitor
            .get("id")
            .or_else(|| monitor.get("name"))
            .and_then(Value::as_str);
        if let Some(id) = id {
            push_dogfood_subject(&mut subjects, &mut seen, "monitor", id);
        }
    }
    if let Some(groups) = query.pointer("/response/groups").and_then(Value::as_array) {
        let mut groups = groups.iter().collect::<Vec<_>>();
        groups.sort_by(|left, right| {
            dogfood_group_count(right)
                .cmp(&dogfood_group_count(left))
                .then_with(|| {
                    string_field(left, "group_hash").cmp(&string_field(right, "group_hash"))
                })
        });
        for group in groups
            .into_iter()
            .take(DOGFOOD_VALUE_ERROR_GROUP_SUBJECT_LIMIT)
        {
            if let Some(id) = group.get("group_hash").and_then(Value::as_str) {
                push_dogfood_subject(&mut subjects, &mut seen, "error_group", id);
            }
        }
    }
    for incident in dogfood_service_incidents(incidents, service) {
        if let Some(id) = incident.get("id").and_then(Value::as_str) {
            push_dogfood_subject(&mut subjects, &mut seen, "incident", id);
        }
    }
    subjects
}

fn push_dogfood_subject(
    subjects: &mut Vec<DogfoodValueSubject>,
    seen: &mut BTreeSet<(String, String)>,
    subject_type: &str,
    subject_id: &str,
) {
    let key = (subject_type.to_owned(), subject_id.to_owned());
    if seen.insert(key.clone()) {
        subjects.push(DogfoodValueSubject {
            subject_type: key.0,
            subject_id: key.1,
        });
    }
}

fn dogfood_group_count(group: &Value) -> u64 {
    ["count", "total_count", "total_errors"]
        .into_iter()
        .find_map(|field| group.get(field).and_then(Value::as_u64))
        .unwrap_or(0)
}

fn dogfood_service_entry(dogfood: &Value, service: &str) -> Option<Value> {
    [
        "/response/surfaces",
        "/response/registry",
        "/response/services",
        "/surfaces",
        "/registry",
        "/services",
    ]
    .into_iter()
    .filter_map(|pointer| dogfood.pointer(pointer).and_then(Value::as_array))
    .flat_map(|items| items.iter())
    .find(|item| string_field(item, "service") == service)
    .cloned()
}

fn dogfood_coverage_verdict(surface: &Value, service: &str, dogfood: &Value) -> String {
    surface
        .get("coverage")
        .or_else(|| surface.get("state"))
        .or_else(|| surface.get("registry_state"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            dogfood_service_state(dogfood, service)
                .as_str()
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "missing".to_owned())
}

fn dogfood_status_target_match(status: &Value, target: &Value) -> Option<Value> {
    status
        .pointer("/response/targets")
        .or_else(|| status.get("targets"))
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| {
                    ["id", "name", "url"].into_iter().any(|field| {
                        string_field(item, field) == string_field(target, field)
                            && !string_field(target, field).is_empty()
                    })
                })
                .cloned()
        })
}

fn dogfood_status_monitor_match(
    status: &Value,
    service: &str,
    monitor: Option<&Value>,
) -> Option<Value> {
    status
        .pointer("/response/monitors")
        .or_else(|| status.get("monitors"))
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| {
                    string_field(item, "service") == service
                        || monitor.is_some_and(|monitor| {
                            ["id", "name"].into_iter().any(|field| {
                                string_field(item, field) == string_field(monitor, field)
                                    && !string_field(monitor, field).is_empty()
                            })
                        })
                })
                .cloned()
        })
}

fn dogfood_health_state(
    status: &Value,
    status_target: Option<&Value>,
    status_monitor: Option<&Value>,
    target: Option<&Value>,
    monitor: Option<&Value>,
) -> String {
    if let Some(state) =
        status_target.and_then(|target| target.get("state").and_then(Value::as_str))
    {
        return state.to_owned();
    }
    if let Some(monitor) = status_monitor {
        if let Some(state) = monitor.get("state").and_then(Value::as_str) {
            return state.to_owned();
        }
        if let Some(status) = monitor.get("last_check_in_status").and_then(Value::as_str) {
            return match status {
                "alive" | "ok" => "up".to_owned(),
                other => other.to_owned(),
            };
        }
    }
    if target.is_some() || monitor.is_some() {
        if status.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return "configured".to_owned();
        }
        return "unavailable".to_owned();
    }
    "missing".to_owned()
}

fn dogfood_service_incidents(incidents: &Value, service: &str) -> Vec<Value> {
    incidents
        .pointer("/response/incidents")
        .or_else(|| incidents.get("incidents"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|incident| string_field(incident, "service") == service)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn dogfood_incident_is_open(incident: &Value) -> bool {
    !matches!(
        string_field(incident, "state").as_str(),
        "resolved" | "closed" | "fixed" | "dismissed"
    )
}

fn dogfood_recent_annotations(annotations: &Value) -> Vec<Value> {
    annotations
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|probe| {
            probe
                .pointer("/probe/response/annotations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .cloned()
        })
        .take(10)
        .collect()
}

fn dogfood_recent_telemetry_events(timeline: &Value) -> Vec<Value> {
    timeline
        .pointer("/response/events")
        .or_else(|| timeline.get("events"))
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter(|event| string_field(event, "event") == "telemetry.event")
                .take(10)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn dogfood_active_claim(claims: &Value, annotations: &Value) -> Value {
    claims
        .as_array()
        .into_iter()
        .chain(annotations.as_array())
        .flatten()
        .find_map(dogfood_probe_active_claim)
        .unwrap_or(Value::Null)
}

fn dogfood_probe_active_claim(probe: &Value) -> Option<Value> {
    probe
        .pointer("/probe/response/current_claim")
        .filter(|claim| claim.is_object())
        .cloned()
        .or_else(|| {
            probe
                .pointer("/probe/response/claims")
                .and_then(Value::as_array)
                .and_then(|claims| {
                    claims
                        .iter()
                        .find(|claim| dogfood_claim_is_active(claim))
                        .cloned()
                })
        })
}

fn dogfood_claim_is_active(claim: &Value) -> bool {
    matches!(
        string_field(claim, "state").as_str(),
        "claimed" | "investigating" | "fix_proposed"
    )
}

fn dogfood_stale_current_work(surface: &Value, total_errors: u64, query: &Value) -> bool {
    if total_errors > 0 || !query.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return false;
    }
    let text = format!(
        "{} {}",
        surface
            .get("failure_mode")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        surface
            .get("next_action")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
    .to_ascii_lowercase();
    ["typeerror", "triage", "flood", "incident"]
        .into_iter()
        .any(|needle| text.contains(needle))
}

fn dogfood_value_verification_status(
    coverage: &str,
    health_ok: bool,
    query_ok: bool,
    stale_current_work: bool,
) -> &'static str {
    if stale_current_work {
        "needs_evidence_refresh"
    } else if coverage == "covered" && health_ok && query_ok {
        "verified"
    } else if !health_ok {
        "health_not_up"
    } else if !query_ok {
        "query_unavailable"
    } else {
        "unproven"
    }
}

fn dogfood_value_state(
    coverage: &str,
    health_ok: bool,
    query_ok: bool,
    stale_current_work: bool,
) -> &'static str {
    if stale_current_work {
        "stale_registry_evidence"
    } else if coverage == "blocked" {
        "blocked"
    } else if coverage == "covered" && health_ok && query_ok {
        "proven"
    } else if coverage == "partial" || health_ok || query_ok {
        "partial"
    } else {
        "unproven"
    }
}

fn dogfood_value_next_action(
    surface: &Value,
    stale_current_work: bool,
    health_state: &str,
    total_errors: u64,
    window: &str,
) -> String {
    let registry_action = surface
        .get("next_action")
        .and_then(Value::as_str)
        .unwrap_or("Add dogfood registry coverage, live health coverage, and query readback.");
    if stale_current_work {
        return format!(
            "Refresh registry evidence: live {window} readback is clean ({total_errors} errors), but the dogfood registry still carries stale triage text."
        );
    }
    if !matches!(health_state, "up" | "missing") {
        return format!(
            "Restore live health state: /api/v1/status reports {health_state}; then rerun the value receipt."
        );
    }
    registry_action.to_owned()
}

fn dogfood_last_verified_outcome(
    service: &str,
    window: &str,
    coverage: &str,
    health_state: &str,
    query_ok: bool,
    total_errors: u64,
    stale_current_work: bool,
) -> String {
    if stale_current_work {
        return format!(
            "{service} live {window} query readback returned {total_errors} errors; registry action needs refresh before it can describe current work."
        );
    }
    if coverage == "covered" && health_state != "missing" && query_ok {
        return format!(
            "{service} is {coverage}, health={health_state}, and live {window} query readback returned {total_errors} errors."
        );
    }
    format!(
        "{service} has coverage={coverage}, health={health_state}, query_ok={query_ok}; value proof is incomplete."
    )
}

fn reconcile_plan_with_live_state(
    plan: &Value,
    target: Option<&Value>,
    monitor: Option<&Value>,
) -> Value {
    let mut plan = plan.clone();
    if let Some(actions) = plan.get_mut("actions").and_then(Value::as_array_mut) {
        for action in actions {
            if action.get("kind").and_then(Value::as_str) == Some("target_enrollment")
                && (target.is_some() || monitor.is_some())
                && let Some(object) = action.as_object_mut()
            {
                object.insert("status".to_owned(), json!("present"));
                object.insert("executor".to_owned(), json!("none"));
                object.insert(
                    "live_evidence".to_owned(),
                    json!({
                        "target_id": target.and_then(|item| item.get("id")).and_then(Value::as_str),
                        "monitor_id": monitor.and_then(|item| item.get("id")).and_then(Value::as_str)
                    }),
                );
            }
        }
    }
    plan
}

fn coverage_mode(framework: &str, health_routes: Option<&Value>) -> &'static str {
    if framework == "nextjs" {
        "nextjs"
    } else if health_routes
        .and_then(Value::as_array)
        .is_some_and(|routes| !routes.is_empty())
    {
        "http"
    } else {
        "static"
    }
}

fn planned_health_url(production_url: Option<&str>, coverage_mode: &str) -> Option<String> {
    production_url.map(|url| {
        let base = url.trim_end_matches('/');
        if coverage_mode == "static" {
            base.to_owned()
        } else {
            format!("{base}/api/health")
        }
    })
}

fn static_site_artifacts(
    service: &str,
    production_url: Option<&str>,
    coverage_mode: &str,
) -> Value {
    if coverage_mode != "static" {
        return Value::Null;
    }
    let health_url = planned_health_url(production_url, coverage_mode);
    json!({
        "mode": "static",
        "target_url": health_url,
        "no_code_path": health_url.map(|url| format!("bin/canary integrate enroll --service {} --url {} --project-root .", shell_arg(service), shell_arg(&url))),
        "vercel_function": {
            "path": "api/health.ts",
            "source": "export default function handler(_req, res) { res.status(200).json({ status: 'ok' }); }\n"
        },
        "browser_capture_warning": "Only use a constrained ingest-only/public key in browser code; never expose admin, read, or server ingest keys.",
        "browser_capture_snippet": format!(
            "<script type=\"module\">import {{ initCanary }} from 'https://esm.sh/@canary-obs/sdk'; import {{ installBrowserErrorObservers }} from 'https://esm.sh/@canary-obs/sdk/nextjs'; initCanary({{ endpoint: window.NEXT_PUBLIC_CANARY_ENDPOINT, apiKey: window.NEXT_PUBLIC_CANARY_API_KEY, service: '{}', environment: 'production' }}); installBrowserErrorObservers();</script>",
            js_string_literal_body(service)
        )
    })
}

fn monitor_templates(service: &str, endpoint: &str) -> Value {
    json!([
        monitor_template(
            service,
            endpoint,
            "cron",
            "schedule",
            3_600_000,
            "Nightly or hourly jobs that should complete on a fixed cadence.",
            ["in_progress", "ok", "error"]
        ),
        monitor_template(
            service,
            endpoint,
            "worker",
            "ttl",
            300_000,
            "Long-running workers that should stay fresh while the process is alive.",
            ["alive", "error"]
        ),
        monitor_template(
            service,
            endpoint,
            "desktop",
            "ttl",
            900_000,
            "Desktop app states that are only live while the user explicitly has work in progress.",
            ["alive", "error"]
        ),
        monitor_template(
            service,
            endpoint,
            "cli",
            "schedule",
            86_400_000,
            "Recurring CLI tasks, audits, or local automation runs.",
            ["in_progress", "ok", "error"]
        )
    ])
}

fn monitor_template<const N: usize>(
    service: &str,
    endpoint: &str,
    runtime: &str,
    mode: &str,
    expected_every_ms: i64,
    description: &str,
    statuses: [&str; N],
) -> Value {
    let monitor = format!("{service}-{runtime}");
    let statuses = statuses.to_vec();
    let create_payload = json!({
        "name": monitor,
        "service": service,
        "mode": mode,
        "expected_every_ms": expected_every_ms
    })
    .to_string();
    let check_in_payload = json!({
        "monitor": monitor,
        "status": statuses[0],
        "summary": format!("{runtime} check-in")
    })
    .to_string();
    json!({
        "runtime": runtime,
        "description": description,
        "monitor": monitor,
        "mode": mode,
        "expected_every_ms": expected_every_ms,
        "create_monitor": format!(
            "curl -fsS -X POST {}/api/v1/monitors -H 'Authorization: Bearer $CANARY_ADMIN_API_KEY' -H 'Content-Type: application/json' -d {}",
            endpoint.trim_end_matches('/'),
            shell_arg(&create_payload)
        ),
        "check_in": format!(
            "curl -fsS -X POST {}/api/v1/check-ins -H 'Authorization: Bearer $CANARY_API_KEY' -H 'Content-Type: application/json' -d {}",
            endpoint.trim_end_matches('/'),
            shell_arg(&check_in_payload)
        ),
        "statuses": statuses
    })
}

fn integration_receipt(root: &Path) -> Result<Value> {
    read_json_file(&root.join(INTEGRATION_RECEIPT_PATH))
}

fn receipt_is_present(receipt: Option<&Value>) -> bool {
    receipt.is_some_and(|value| !value.is_null())
}

fn write_integration_receipt(root: &Path, value: &Value) -> Result<Value> {
    let path = root.join(INTEGRATION_RECEIPT_PATH);
    write_json_file(&path, value)?;
    Ok(json!({"path": path.display().to_string(), "status": "updated"}))
}

fn patch_integration_receipt_value(root: &Path, input: &IntegrationInput, plan: &Value) -> Value {
    let mut planned = integration_receipt_value(input, plan, None);
    let Ok(existing) = integration_receipt(root) else {
        return planned;
    };
    if existing.get("service") != planned.get("service")
        || existing.get("verification_status").and_then(Value::as_str) != Some("verified")
    {
        return planned;
    }

    for field in [
        "target_id",
        "monitor_ids",
        "webhook_ids",
        "api_key_id",
        "verification_status",
        "last_verified_at",
    ] {
        if let Some(value) = existing.get(field).filter(|value| !value.is_null()) {
            planned[field] = value.clone();
        }
    }
    if planned.get("health_url").is_some_and(Value::is_null)
        && let Some(value) = existing.get("health_url").filter(|value| !value.is_null())
    {
        planned["health_url"] = value.clone();
    }
    planned
}

fn integration_receipt_value(
    input: &IntegrationInput,
    plan: &Value,
    enroll: Option<&Value>,
) -> Value {
    let service = plan
        .get("service")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let coverage_mode = plan
        .get("coverage_mode")
        .and_then(Value::as_str)
        .unwrap_or("nextjs");
    let health_url = planned_health_url(input.production_url.as_deref(), coverage_mode);
    let target_id = enroll
        .and_then(|value| value.pointer("/target/id"))
        .and_then(Value::as_str);
    let api_key_id = enroll
        .and_then(|value| value.pointer("/api_key/id"))
        .and_then(Value::as_str);

    json!({
        "schema_version": 1,
        "service": service,
        "environment": "production",
        "canary_endpoint": input.endpoint,
        "health_url": health_url,
        "target_id": target_id,
        "monitor_ids": [],
        "webhook_ids": [],
        "api_key_id": api_key_id,
        "verification_status": if target_id.is_some() || api_key_id.is_some() { "verified" } else { "planned" },
        "env_names": [
            DEFAULT_INTEGRATION_ENDPOINT_ENV,
            DEFAULT_INTEGRATION_SERVER_KEY_ENV,
            DEFAULT_INTEGRATION_PUBLIC_ENDPOINT_ENV,
            DEFAULT_INTEGRATION_PUBLIC_KEY_ENV
        ],
        "verification_commands": [
            format!("bin/canary integrate status {} --service {} --json", shell_arg(&input.target.display().to_string()), shell_arg(service)),
            format!("bin/canary errors {} --window 1h --json", shell_arg(service))
        ],
        "last_verified_at": now_unix_timestamp_string()
    })
}

fn enrollment_receipt_value(
    endpoint: &str,
    request: &IntegrationEnrollRequest,
    enroll: &Value,
) -> Value {
    let target_id = enroll.pointer("/target/id").and_then(Value::as_str);
    let api_key_id = enroll.pointer("/api_key/id").and_then(Value::as_str);

    json!({
        "schema_version": 1,
        "service": request.service,
        "environment": request.environment,
        "canary_endpoint": endpoint,
        "health_url": request.url,
        "target_id": target_id,
        "monitor_ids": [],
        "webhook_ids": [],
        "api_key_id": api_key_id,
        "verification_status": "verified",
        "env_names": [
            DEFAULT_INTEGRATION_ENDPOINT_ENV,
            DEFAULT_INTEGRATION_SERVER_KEY_ENV,
            DEFAULT_INTEGRATION_PUBLIC_ENDPOINT_ENV,
            DEFAULT_INTEGRATION_PUBLIC_KEY_ENV
        ],
        "verification_commands": [
            format!("bin/canary integrate status . --service {} --json", shell_arg(&request.service)),
            format!("bin/canary errors {} --window 1h --json", shell_arg(&request.service))
        ],
        "last_verified_at": now_unix_timestamp_string()
    })
}

fn now_unix_timestamp_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}

fn integration_coverage_verdict(
    service: &str,
    discovery: &Value,
    targets: &Value,
    monitors: &Value,
    webhooks: &Value,
    query: &Value,
    dogfood: &Value,
) -> Value {
    let local_capture = discovery
        .pointer("/signals/canary_present")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || discovery
            .pointer("/signals/canary_sdk_dependency")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || receipt_is_present(discovery.get("integration_receipt"));
    let target = live_collection_contains_service(targets, "targets", service);
    let monitor = live_collection_contains_service(monitors, "monitors", service);
    let webhook = live_collection_non_empty(webhooks, "webhooks");
    let query_readback = query.get("ok").and_then(Value::as_bool).unwrap_or(false)
        && query
            .pointer("/response/service")
            .and_then(Value::as_str)
            .is_none_or(|queried_service| queried_service == service);
    let dogfood_state = dogfood_service_state(dogfood, service);
    let mut failures = Vec::new();

    if !local_capture {
        failures.push(json!({"kind": "local_capture", "message": "no local Canary SDK, bespoke capture, or integration receipt found"}));
    }
    if !target && !monitor {
        failures.push(json!({"kind": "health_coverage", "message": "no live target or monitor matched the service"}));
    }
    if !query_readback {
        failures.push(
            json!({"kind": "query_readback", "message": "Canary query readback was unavailable"}),
        );
    }

    let status = if failures.is_empty() {
        "covered"
    } else if local_capture || target || monitor {
        "partial"
    } else {
        "missing"
    };

    json!({
        "status": status,
        "local_capture": local_capture,
        "target_enrolled": target,
        "monitor_enrolled": monitor,
        "webhook_configured": webhook,
        "query_readback": query_readback,
        "dogfood_state": dogfood_state,
        "strict_failures": failures
    })
}

fn live_collection_contains_service(probe: &Value, key: &str, service: &str) -> bool {
    live_collection_service_match(probe, key, service).is_some()
}

fn live_collection_service_match(probe: &Value, key: &str, service: &str) -> Option<Value> {
    probe
        .pointer(&format!("/response/{key}"))
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| {
                    string_field(item, "service") == service
                        || string_field(item, "name") == service
                })
                .cloned()
        })
}

fn live_collection_non_empty(probe: &Value, key: &str) -> bool {
    probe
        .pointer(&format!("/response/{key}"))
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
}

fn dogfood_service_state(dogfood: &Value, service: &str) -> Value {
    [
        "/response/services",
        "/response/registry",
        "/response/surfaces",
    ]
    .into_iter()
    .filter_map(|pointer| dogfood.pointer(pointer).and_then(Value::as_array))
    .flat_map(|items| items.iter())
    .find(|item| string_field(item, "service") == service)
    .map(|item| {
        item.get("state")
            .or_else(|| item.get("registry_state"))
            .or_else(|| item.get("coverage"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned()
    })
    .map_or(Value::Null, Value::String)
}

/// Summarize an integration discovery/plan/patch/enroll response.
pub fn summarize_integration(value: &Value) -> Vec<String> {
    let mut lines = vec![
        format!("service: {}", string_field(value, "service")),
        format!("framework: {}", string_field(value, "framework")),
    ];
    if let Some(coverage) = value.get("coverage") {
        lines.push(format!("coverage: {}", string_field(coverage, "status")));
        lines.push(format!(
            "strict_failures: {}",
            array_len(coverage, "strict_failures")
        ));
    } else if let Some(actions) = value.get("actions").and_then(Value::as_array) {
        lines.push(format!("actions: {}", actions.len()));
        lines.extend(actions.iter().map(|action| {
            format!(
                "- {}: {}",
                string_field(action, "kind"),
                string_field(action, "status")
            )
        }));
    } else if let Some(changes) = value.get("changes").and_then(Value::as_array) {
        lines.push(format!("changes: {}", changes.len()));
        lines.extend(changes.iter().map(|change| {
            format!(
                "- {}: {}",
                string_field(change, "path"),
                string_field(change, "status")
            )
        }));
    } else if value.get("target").is_some() && value.get("api_key").is_some() {
        lines.push(format!(
            "target: {}",
            value
                .get("target")
                .map(|target| string_field(target, "id"))
                .unwrap_or_else(|| "unknown".to_owned())
        ));
        lines.push(format!(
            "api_key: {}",
            value
                .get("api_key")
                .map(|key| string_field(key, "key"))
                .unwrap_or_else(|| "redacted".to_owned())
        ));
    }
    lines
}

fn read_json_file(path: &Path) -> Result<Value> {
    let body = fs::read_to_string(path).map_err(|source| CliError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&body).map_err(CliError::from)
}

fn file_name_string(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn vercel_project_name(root: &Path) -> Option<String> {
    let path = root.join(".vercel/project.json");
    read_json_file(&path)
        .ok()
        .and_then(|value| string_value(&value, "projectName"))
}

fn service_name_from_project(project: &str) -> String {
    project
        .trim()
        .trim_start_matches("www.")
        .replace("_", "-")
        .to_ascii_lowercase()
}

fn detect_package_manager(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else if root.join("package-lock.json").is_file() {
        "npm"
    } else {
        "unknown"
    }
}

fn detect_framework(root: &Path, package: Option<&Value>) -> &'static str {
    if package_has_dependency(package, "next")
        || root.join("next.config.js").is_file()
        || root.join("next.config.mjs").is_file()
        || root.join("next.config.ts").is_file()
    {
        "nextjs"
    } else if package.is_some() {
        "node"
    } else {
        "unknown"
    }
}

fn detect_platform(root: &Path) -> &'static str {
    if root.join(".vercel/project.json").is_file() {
        "vercel"
    } else if root.join("fly.toml").is_file() {
        "fly"
    } else {
        "unknown"
    }
}

fn discover_platform_env(root: &Path, platform: &str) -> Value {
    match platform {
        "vercel" => discover_vercel_env(root),
        "fly" => json!({
            "ok": false,
            "source": "fly",
            "reason": "fly env-name listing is not implemented"
        }),
        _ => json!({
            "ok": false,
            "source": platform,
            "reason": "unsupported platform"
        }),
    }
}

fn discover_vercel_env(root: &Path) -> Value {
    let output = match Command::new("vercel")
        .current_dir(root)
        .arg("env")
        .arg("list")
        .arg("production")
        .arg("--format")
        .arg("json")
        .arg("--cwd")
        .arg(root)
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            return json!({
                "ok": false,
                "source": "vercel",
                "reason": error.to_string()
            });
        }
    };
    if !output.status.success() {
        return json!({
            "ok": false,
            "source": "vercel",
            "status": output
                .status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
            "stderr": redact_text(&String::from_utf8_lossy(&output.stderr))
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let env_names = match vercel_env_names_from_stdout(&stdout) {
        Ok(names) => names,
        Err(reason) => {
            return json!({
                "ok": false,
                "source": "vercel",
                "reason": reason
            });
        }
    };

    json!({
        "ok": true,
        "source": "vercel",
        "environment": "production",
        "env_names": env_names
    })
}

fn vercel_env_names_from_stdout(stdout: &str) -> std::result::Result<Vec<String>, String> {
    let Some(json_start) = stdout.find('{') else {
        return Err("vercel env list did not return JSON".to_owned());
    };
    let mut deserializer = serde_json::Deserializer::from_str(&stdout[json_start..]);
    let parsed = Value::deserialize(&mut deserializer).map_err(|error| error.to_string())?;
    Ok(parsed
        .get("envs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("key").and_then(Value::as_str))
        .filter(|name| valid_env_name(name))
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>())
}

fn package_has_dependency(package: Option<&Value>, name: &str) -> bool {
    package.is_some_and(|package| {
        [
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ]
        .into_iter()
        .any(|key| package.get(key).and_then(|deps| deps.get(name)).is_some())
    })
}

fn discover_env_names(root: &Path) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    for filename in [
        ".env",
        ".env.local",
        ".env.production",
        ".env.production.local",
        ".env.example",
    ] {
        let path = root.join(filename);
        if !path.is_file() {
            continue;
        }
        let body = fs::read_to_string(&path).map_err(|source| CliError::Io {
            path: path.clone(),
            source,
        })?;
        for line in body.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') || trimmed.is_empty() {
                continue;
            }
            if let Some((name, _value)) = trimmed.split_once('=') {
                let name = name.trim();
                if valid_env_name(name) {
                    names.insert(name.to_owned());
                }
            }
        }
    }
    Ok(names.into_iter().collect())
}

fn merge_env_names(local: &[String], receipt: &[String]) -> Vec<String> {
    local
        .iter()
        .chain(receipt.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit())
}

fn discover_code_paths(root: &Path) -> Result<Vec<Value>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    let mut queue = VecDeque::from([root.to_path_buf()]);
    let mut scanned = 0usize;
    while let Some(dir) = queue.pop_front() {
        if scanned > 500 {
            break;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(source) => {
                return Err(CliError::Io { path: dir, source });
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| CliError::Io {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".git")
                || matches!(
                    name.as_ref(),
                    "node_modules" | ".next" | "dist" | "build" | "coverage" | "target"
                )
            {
                continue;
            }
            if path.is_dir() && !path.is_symlink() {
                queue.push_back(path);
                continue;
            }
            scanned += 1;
            if !is_source_like(&path) {
                continue;
            }
            let body = fs::read_to_string(&path).unwrap_or_default();
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path.as_path())
                .display()
                .to_string();
            if body.contains("@canary-obs/sdk")
                || body.contains("initCanary")
                || body.contains("CANARY_API_KEY")
                || body.contains("CANARY_ENDPOINT")
                || body.contains("NEXT_PUBLIC_CANARY_API_KEY")
                || body.contains("NEXT_PUBLIC_CANARY_ENDPOINT")
                || body.contains("/api/v1/errors")
                || body.contains("/api/v1/check-ins")
            {
                paths.push(json!({"kind": "canary", "path": rel}));
            } else if body.contains("@sentry/")
                || body.contains("Sentry.")
                || body.contains("withSentryConfig")
            {
                paths.push(json!({"kind": "sentry", "path": rel}));
            }
        }
    }
    Ok(paths)
}

fn is_source_like(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"))
}

fn discover_health_routes(root: &Path) -> Vec<Value> {
    let mut routes = BTreeSet::new();
    for (path, route) in [
        ("app/api/health/route.ts", "/api/health"),
        ("app/api/health/route.tsx", "/api/health"),
        ("src/app/api/health/route.ts", "/api/health"),
        ("src/app/api/health/route.tsx", "/api/health"),
        ("pages/api/health.ts", "/api/health"),
        ("pages/api/health.js", "/api/health"),
        ("src/pages/api/health.ts", "/api/health"),
        ("src/pages/api/health.js", "/api/health"),
        ("app/health/route.ts", "/health"),
        ("app/health/route.tsx", "/health"),
        ("src/app/health/route.ts", "/health"),
        ("src/app/health/route.tsx", "/health"),
    ] {
        if root.join(path).is_file() {
            routes.insert((path.to_owned(), route.to_owned()));
        }
    }
    for app_root in ["app", "src/app"] {
        discover_app_health_route_groups(root, app_root, &mut routes);
    }
    routes
        .into_iter()
        .map(|(path, route)| json!({"path": path, "route": route}))
        .collect()
}

fn discover_app_health_route_groups(
    root: &Path,
    app_root: &str,
    routes: &mut BTreeSet<(String, String)>,
) {
    let base = root.join(app_root);
    if !base.is_dir() {
        return;
    }
    let mut queue = VecDeque::from([base]);
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                queue.push_back(path);
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !matches!(
                file_name,
                "route.ts" | "route.tsx" | "route.js" | "route.jsx"
            ) {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let parts = relative
                .iter()
                .filter_map(|part| part.to_str())
                .collect::<Vec<_>>();
            if parts.len() < 5
                || parts[parts.len() - 3] != "api"
                || parts[parts.len() - 2] != "health"
            {
                continue;
            }
            let Some(app_index) = parts.iter().position(|part| *part == "app") else {
                continue;
            };
            let api_index = parts.len() - 3;
            let route_prefix = parts
                .iter()
                .skip(app_index + 1)
                .take(api_index.saturating_sub(app_index + 1))
                .filter(|part| !part.starts_with('(') && !part.ends_with(')'))
                .map(|part| format!("/{part}"))
                .collect::<String>();
            routes.insert((
                relative.display().to_string(),
                format!("{route_prefix}/api/health"),
            ));
        }
    }
}

fn file_contains(path: &Path, needle: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|body| body.contains(needle))
}

fn is_next_error_boundary_path(path: &str) -> bool {
    path == "app/global-error.tsx"
        || path == "app/global-error.jsx"
        || path.ends_with("/global-error.tsx")
        || path.ends_with("/global-error.jsx")
        || path.ends_with("/error.tsx")
        || path.ends_with("/error.jsx")
}

fn plan_action(kind: &str, status: &str, description: &str, executor: &str) -> Value {
    json!({
        "kind": kind,
        "status": status,
        "description": description,
        "executor": executor
    })
}

fn required_env_names_present(env_set: &BTreeSet<&str>) -> &'static str {
    let required = [
        DEFAULT_INTEGRATION_ENDPOINT_ENV,
        DEFAULT_INTEGRATION_SERVER_KEY_ENV,
        DEFAULT_INTEGRATION_PUBLIC_ENDPOINT_ENV,
        DEFAULT_INTEGRATION_PUBLIC_KEY_ENV,
    ];
    if required.into_iter().all(|name| env_set.contains(name)) {
        "present"
    } else {
        "needed"
    }
}

fn shell_arg(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '-' | '_' | ':' | '='))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn js_string_literal_body(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn instrumentation_path(root: &Path) -> PathBuf {
    root.join("instrumentation.ts")
}

fn health_route_path(root: &Path) -> PathBuf {
    root.join("app/api/health/route.ts")
}

fn global_error_path(root: &Path) -> PathBuf {
    root.join("app/global-error.tsx")
}

fn instrumentation_source(service: &str, endpoint: &str) -> String {
    let endpoint = ts_string_literal(endpoint);
    let service = ts_string_literal(service);
    format!(
        "import {{ initCanary }} from \"@canary-obs/sdk\";\nexport {{ onRequestError }} from \"@canary-obs/sdk/nextjs\";\n\nexport function register() {{\n  initCanary({{\n    endpoint: process.env.{DEFAULT_INTEGRATION_ENDPOINT_ENV} ?? {endpoint},\n    apiKey: process.env.{DEFAULT_INTEGRATION_SERVER_KEY_ENV} ?? \"\",\n    service: {service},\n    environment: process.env.NODE_ENV ?? \"production\",\n    scrubPii: true,\n  }});\n}}\n"
    )
}

fn health_route_source() -> &'static str {
    "// Canary health route generated by canary integrate patch.\nexport function GET() {\n  return Response.json({ status: \"ok\" });\n}\n"
}

fn global_error_source(service: &str) -> String {
    let service = ts_string_literal(service);
    format!(
        "\"use client\";\n\nimport {{ useEffect }} from \"react\";\nimport {{ captureException, initCanary }} from \"@canary-obs/sdk\";\n\nexport default function GlobalError({{ error }}: {{ error: Error }}) {{\n  useEffect(() => {{\n    initCanary({{\n      endpoint: process.env.{DEFAULT_INTEGRATION_PUBLIC_ENDPOINT_ENV} ?? \"\",\n      apiKey: process.env.{DEFAULT_INTEGRATION_PUBLIC_KEY_ENV} ?? \"\",\n      service: process.env.NEXT_PUBLIC_CANARY_SERVICE ?? {service},\n      environment: process.env.NODE_ENV ?? \"production\",\n      scrubPii: true,\n    }});\n    void captureException(error);\n  }}, [error]);\n\n  return (\n    <html>\n      <body>Something went wrong.</body>\n    </html>\n  );\n}}\n"
    )
}

fn ts_string_literal(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

fn update_package_dependency(root: &Path) -> Result<Value> {
    let path = root.join("package.json");
    let mut package = read_json_file(&path)?;
    if package_has_dependency(Some(&package), "@canary-obs/sdk") {
        return Ok(json!({"path": path.display().to_string(), "status": "unchanged"}));
    }
    let Some(object) = package.as_object_mut() else {
        return Err(CliError::Message(
            "package.json must be a JSON object".to_owned(),
        ));
    };
    let deps = object.entry("dependencies").or_insert_with(|| json!({}));
    let Some(deps) = deps.as_object_mut() else {
        return Err(CliError::Message(
            "package.json dependencies must be a JSON object".to_owned(),
        ));
    };
    deps.insert("@canary-obs/sdk".to_owned(), json!("^0.1.0"));
    write_json_file(&path, &package)?;
    Ok(json!({"path": path.display().to_string(), "status": "updated"}))
}

fn write_json_file(path: &Path, value: &Value) -> Result<()> {
    let body = format!("{}\n", serde_json::to_string_pretty(value)?);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CliError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, body).map_err(|source| CliError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write_if_absent_or_canary(path: &Path, body: &str) -> Result<Value> {
    if path.exists() {
        let existing = fs::read_to_string(path).map_err(|source| CliError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if !existing.contains("@canary-obs/sdk")
            && !existing.to_ascii_lowercase().contains("canary")
        {
            return Ok(json!({
                "path": path.display().to_string(),
                "status": "skipped",
                "reason": "existing file is not Canary-owned"
            }));
        }
        if existing == body {
            return Ok(json!({"path": path.display().to_string(), "status": "unchanged"}));
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CliError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, body).map_err(|source| CliError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(json!({"path": path.display().to_string(), "status": "updated"}))
}

fn redact_secret_value(mut value: Value) -> Value {
    if let Some(key) = value.get_mut("api_key").and_then(Value::as_object_mut) {
        if key.contains_key("key") {
            key.insert("key".to_owned(), json!("redacted"));
        }
        if key.contains_key("warning") {
            key.insert(
                "warning".to_owned(),
                json!("Key redacted by CLI. Re-run with --show-secret only in a secure handoff."),
            );
        }
    }
    redact_secret_strings(&mut value);
    value
}

fn redact_secret_strings(value: &mut Value) {
    match value {
        Value::String(text) => {
            *text = redact_text(text);
        }
        Value::Array(items) => {
            for item in items {
                redact_secret_strings(item);
            }
        }
        Value::Object(object) => {
            for item in object.values_mut() {
                redact_secret_strings(item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
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
    let witness_runs = witness_run_references(repo_root);
    let dogfood = match run_dogfood_inventory(repo_root, false) {
        Ok(value) => json!({"ok": true, "summary": summarize_dogfood(&value), "response": value}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    };
    let dogfood_value = dogfood
        .get("response")
        .map(|response| {
            let summary = dogfood_value_summary(response);
            json!({"ok": true, "summary": summarize_dogfood_value(&summary), "response": summary})
        })
        .unwrap_or_else(|| json!({"ok": false, "error": string_field(&dogfood, "error")}));
    let dr = dr_evidence_report(repo_root);
    let worker_readiness = worker_readiness_report(&readyz);
    let alert_plane = alert_plane_report(&worker_readiness);
    let verdict = doctor_verdict(
        &healthz,
        &readyz,
        &report,
        &incidents,
        &canary_errors,
        &witness,
        &worker_readiness,
        &alert_plane,
        &dogfood,
        &witness_runs,
        current_unix_ms(),
    );

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
        "dr": dr,
        "dogfood": dogfood,
        "dogfood_value": dogfood_value,
        "worker_readiness": worker_readiness,
        "alert_plane": alert_plane,
        "verdict": verdict
    })
}

/// Render doctor text.
pub fn summarize_doctor(value: &Value) -> Vec<String> {
    let mut lines = vec![
        format!("endpoint: {}", string_field(value, "endpoint")),
        format!("key: {}", string_field(value, "key")),
        format!("key_scope: {}", string_field(value, "key_scope")),
    ];
    lines.push(format!(
        "verdict: {}",
        verdict_summary(value.get("verdict"))
    ));
    if let Some(blocking) = value
        .get("verdict")
        .and_then(|verdict| verdict.get("blocking_signals"))
        .and_then(Value::as_array)
        .filter(|signals| !signals.is_empty())
    {
        let signals = blocking
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("verdict_blocking: {signals}"));
    }
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
    lines.push(format!("dr: {}", dr_summary(value.get("dr"))));
    lines.push(format!(
        "canary_errors: {}",
        probe_summary(value.get("canary_errors"))
    ));
    lines.push(format!(
        "incidents: {}",
        probe_summary(value.get("incidents"))
    ));
    lines.push(format!("dogfood: {}", probe_summary(value.get("dogfood"))));
    lines.push(format!(
        "dogfood_value: {}",
        probe_summary(value.get("dogfood_value"))
    ));
    lines.push(format!(
        "worker_readiness: {}",
        worker_readiness_summary(value.get("worker_readiness"))
    ));
    lines.push(format!(
        "alert_plane: {}",
        alert_plane_summary(value.get("alert_plane"))
    ));
    lines
}

#[allow(clippy::too_many_arguments)]
fn doctor_verdict(
    healthz: &Value,
    readyz: &Value,
    summary: &Value,
    incidents: &Value,
    canary_errors: &Value,
    witness: &Value,
    worker_readiness: &Value,
    alert_plane: &Value,
    dogfood: &Value,
    receipt_run_references: &Value,
    now_unix_ms: u64,
) -> Value {
    let mut blocking_signals = Vec::new();
    let mut unable = false;

    if !probe_ok(healthz) {
        unable = true;
        blocking_signals.push(format!(
            "/healthz unavailable: {}",
            string_field(healthz, "error")
        ));
    }
    if !probe_ok(readyz) {
        unable = true;
        blocking_signals.push(format!(
            "/readyz unavailable: {}",
            string_field(readyz, "error")
        ));
    }
    if !probe_ok(summary) {
        unable = true;
        blocking_signals.push(format!(
            "authenticated summary unavailable: {}",
            string_field(summary, "error")
        ));
    }
    if !probe_ok(canary_errors) {
        unable = true;
        blocking_signals.push(format!(
            "canary error readback unavailable: {}",
            string_field(canary_errors, "error")
        ));
    }
    if !probe_ok(incidents) {
        unable = true;
        blocking_signals.push(format!(
            "incident readback unavailable: {}",
            string_field(incidents, "error")
        ));
    }

    let witness_age_ms = witness_age_ms(witness, now_unix_ms);
    if let Some(signal) = witness_blocking_signal(witness, witness_age_ms) {
        blocking_signals.push(signal);
    }
    if string_field(witness, "status") == "unavailable" {
        unable = true;
    }

    let open_canary_incident = open_canary_incident(incidents);
    if let Some(incident) = &open_canary_incident {
        let id = string_field(incident, "id");
        let title = incident
            .get("title")
            .or_else(|| incident.get("summary"))
            .and_then(Value::as_str)
            .unwrap_or("untitled");
        blocking_signals.push(format!("open Canary incident {id}: {title}"));
    }

    let worker_pressure = worker_pressure_report(worker_readiness);
    if !worker_readiness
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        unable = true;
        blocking_signals.push(format!(
            "worker readiness unavailable: {}",
            string_field(worker_readiness, "reason")
        ));
    }
    if !alert_plane
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        unable = true;
        blocking_signals.push(format!(
            "alert-plane unavailable: {}",
            string_field(alert_plane, "reason")
        ));
    } else if string_field(alert_plane, "status") != "healthy" {
        blocking_signals.push(format!(
            "alert-plane impaired: {}",
            alert_plane_reason_summary(alert_plane)
        ));
    }
    let failing_workers = worker_pressure
        .get("failing_workers")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let pressured_workers = worker_pressure
        .get("pressured_workers")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if failing_workers > 0 {
        blocking_signals.push(format!(
            "{} worker(s) failing: {}",
            failing_workers,
            worker_names(&worker_pressure, "failing")
        ));
    }

    let canary_error_total = canary_error_total(canary_errors);
    if canary_error_total > 0 {
        blocking_signals.push(format!(
            "{canary_error_total} recent service=canary error(s)"
        ));
    }

    let dogfood_gap_count = dogfood_gap_count(dogfood);
    let overall = if unable {
        "unable"
    } else if blocking_signals.is_empty() {
        "healthy"
    } else {
        "degraded"
    };
    let next_operator_action = next_operator_action(
        overall,
        witness,
        failing_workers,
        pressured_workers,
        canary_error_total,
        dogfood_gap_count,
        open_canary_incident.as_ref(),
    );

    json!({
        "overall": overall,
        "blocking_signals": blocking_signals,
        "next_operator_action": next_operator_action,
        "witness_age_ms": witness_age_ms,
        "open_canary_incident": open_canary_incident.unwrap_or(Value::Null),
        "alert_plane": alert_plane,
        "worker_pressure": worker_pressure,
        "dogfood_gap_count": dogfood_gap_count,
        "receipt_run_references": receipt_run_references
    })
}

fn next_operator_action(
    overall: &str,
    witness: &Value,
    failing_workers: u64,
    pressured_workers: u64,
    canary_error_total: u64,
    dogfood_gap_count: usize,
    open_canary_incident: Option<&Value>,
) -> String {
    if overall == "unable" {
        return "Restore `bin/canary doctor --json` evidence first: verify `/healthz`, `/readyz`, and read/admin API credentials, then rerun the doctor.".to_owned();
    }
    if witness_needs_operator(witness) {
        return "Run `gh workflow run \"Canary Witness\" --ref master`; then inspect the latest witness receipt and rerun `bin/canary doctor --json`.".to_owned();
    }
    if let Some(incident) = open_canary_incident {
        return format!(
            "Investigate open Canary incident {}; inspect `bin/canary incidents --open --json` and rerun `bin/canary doctor --json` after remediation.",
            string_field(incident, "id")
        );
    }
    if failing_workers > 0 || pressured_workers > 0 {
        return "Inspect alert-plane worker pressure and drain the named backlog before rerunning `bin/canary doctor --json`.".to_owned();
    }
    if canary_error_total > 0 {
        return "Run `bin/canary errors canary --window 1h --json`, fix the newest error class, and rerun `bin/canary doctor --json`.".to_owned();
    }
    if dogfood_gap_count > 0 {
        return "No runtime blocker; run `bin/canary dogfood audit --strict --json` and close the reported coverage gaps.".to_owned();
    }
    "No immediate operator action; keep the external witness and scheduled deploy monitors running."
        .to_owned()
}

fn witness_blocking_signal(witness: &Value, witness_age_ms: Option<u64>) -> Option<String> {
    let monitor = string_field(witness, "monitor");
    match string_field(witness, "status").as_str() {
        "observed" => {
            let state = string_field(witness, "state");
            if state == "up" {
                return None;
            }
            let last_status = string_field(witness, "last_check_in_status");
            Some(match witness_age_ms {
                Some(age) => {
                    format!("{monitor} {state}; last {last_status} check-in was {age} ms ago")
                }
                None => format!("{monitor} {state}; last {last_status} check-in age unknown"),
            })
        }
        "configured" => Some(format!(
            "{monitor} configured but no external check-in has been observed"
        )),
        "missing" => Some(format!("{monitor} monitor is missing")),
        "unavailable" => Some(format!("{monitor} monitor readback is unavailable")),
        other => Some(format!("{monitor} witness status is {other}")),
    }
}

fn witness_needs_operator(witness: &Value) -> bool {
    match string_field(witness, "status").as_str() {
        "observed" => string_field(witness, "state") != "up",
        "configured" | "missing" | "unavailable" => true,
        _ => true,
    }
}

fn witness_age_ms(witness: &Value, now_unix_ms: u64) -> Option<u64> {
    let timestamp = witness
        .get("last_check_in_at")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && *value != "unknown")?;
    let observed = rfc3339_unix_ms(timestamp)?;
    Some(now_unix_ms.saturating_sub(observed))
}

fn rfc3339_unix_ms(input: &str) -> Option<u64> {
    let parsed = OffsetDateTime::parse(input, &Rfc3339).ok()?;
    let seconds = u64::try_from(parsed.unix_timestamp()).ok()?;
    seconds
        .checked_mul(1_000)?
        .checked_add(u64::from(parsed.nanosecond() / 1_000_000))
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn open_canary_incident(incidents: &Value) -> Option<Value> {
    incidents
        .get("response")
        .and_then(|response| response.get("incidents"))
        .or_else(|| incidents.get("incidents"))
        .and_then(Value::as_array)?
        .iter()
        .find(|incident| {
            incident.get("service").and_then(Value::as_str) == Some("canary")
                && incident_is_open(incident)
        })
        .cloned()
}

fn incident_is_open(incident: &Value) -> bool {
    !matches!(
        incident
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("open"),
        "resolved" | "closed" | "dismissed"
    )
}

fn canary_error_total(canary_errors: &Value) -> u64 {
    canary_errors
        .get("response")
        .and_then(|response| response.get("total_errors"))
        .or_else(|| canary_errors.get("total_errors"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn dogfood_gap_count(dogfood: &Value) -> usize {
    dogfood.get("response").map_or_else(
        || dogfood_strict_failure_count(dogfood),
        dogfood_strict_failure_count,
    )
}

fn worker_pressure_report(worker_readiness: &Value) -> Value {
    if !worker_readiness
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return json!({
            "status": "unavailable",
            "pressured_workers": 0,
            "failing_workers": 0,
            "reason": string_field(worker_readiness, "reason"),
            "workers": []
        });
    }

    let workers = worker_readiness
        .get("workers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let pressured = workers
        .iter()
        .filter(|worker| worker_is_pressured(worker))
        .map(worker_pressure_detail)
        .collect::<Vec<_>>();
    let failing = workers
        .iter()
        .filter(|worker| worker_is_failing(worker))
        .map(worker_pressure_detail)
        .collect::<Vec<_>>();
    let status = if !failing.is_empty() {
        "failing"
    } else if !pressured.is_empty() {
        "pressured"
    } else {
        worker_readiness
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("ready")
    };

    json!({
        "status": status,
        "pressured_workers": pressured.len(),
        "failing_workers": failing.len(),
        "workers": pressured,
        "failing": failing
    })
}

fn alert_plane_report(worker_readiness: &Value) -> Value {
    if !worker_readiness
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return json!({
            "available": false,
            "status": "unavailable",
            "reason": string_field(worker_readiness, "reason"),
            "worker_count": 0,
            "impaired_workers": 0,
            "workers": [],
            "reasons": []
        });
    }

    let workers = worker_readiness
        .get("workers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let worker_count = worker_readiness
        .get("worker_count")
        .and_then(Value::as_u64)
        .unwrap_or(workers.len() as u64);
    let impaired = workers
        .iter()
        .filter(|worker| alert_worker_is_impaired(worker))
        .map(alert_plane_detail)
        .collect::<Vec<_>>();
    let reasons = impaired.iter().map(alert_plane_reason).collect::<Vec<_>>();
    let status = if impaired.is_empty() {
        "healthy"
    } else {
        "impaired"
    };

    json!({
        "available": true,
        "status": status,
        "worker_count": worker_count,
        "impaired_workers": impaired.len(),
        "workers": impaired,
        "reasons": reasons
    })
}

fn alert_plane_detail(worker: &Value) -> Value {
    json!({
        "name": string_field(worker, "name"),
        "state": string_field(worker, "state"),
        "health": string_field(worker, "health"),
        "failure_count": worker.get("failure_count").and_then(Value::as_u64).unwrap_or(0),
        "consecutive_failures": worker.get("consecutive_failures").and_then(Value::as_u64).unwrap_or(0),
        "due_count": worker.get("due_count").and_then(Value::as_u64).unwrap_or(0),
        "in_flight_count": worker.get("in_flight_count").and_then(Value::as_u64).unwrap_or(0),
        "oldest_due_age_ms": worker.get("oldest_due_age_ms").cloned().unwrap_or(Value::Null),
        "backoff_or_circuit_open": worker.get("backoff_or_circuit_open").and_then(Value::as_bool).unwrap_or(false),
        "reason": alert_worker_reason(worker)
    })
}

fn alert_worker_is_impaired(worker: &Value) -> bool {
    worker.get("state").and_then(Value::as_str) != Some("started")
        || worker.get("health").and_then(Value::as_str) != Some("ok")
        || worker
            .get("failure_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        || worker
            .get("consecutive_failures")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        || worker
            .get("backoff_or_circuit_open")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn alert_worker_reason(worker: &Value) -> String {
    let name = string_field(worker, "name");
    let state = string_field(worker, "state");
    let health = string_field(worker, "health");
    if state != "started" {
        return format!("{name} {state}");
    }
    if worker
        .get("backoff_or_circuit_open")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return format!("{name} backoff_or_circuit_open");
    }
    if health != "ok" {
        return format!("{name} {health}");
    }
    let consecutive_failures = worker
        .get("consecutive_failures")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if consecutive_failures > 0 {
        return format!("{name} consecutive_failures={consecutive_failures}");
    }
    let failure_count = worker
        .get("failure_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if failure_count > 0 {
        return format!("{name} failure_count={failure_count}");
    }
    format!("{name} impaired")
}

fn alert_plane_reason(worker: &Value) -> String {
    worker
        .get("reason")
        .and_then(Value::as_str)
        .map_or_else(|| alert_worker_reason(worker), ToOwned::to_owned)
}

fn alert_plane_reason_summary(alert_plane: &Value) -> String {
    let reasons = alert_plane
        .get("reasons")
        .and_then(Value::as_array)
        .map(|reasons| reasons.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    if reasons.is_empty() {
        "unknown".to_owned()
    } else {
        reasons.join(", ")
    }
}

fn worker_names(worker_pressure: &Value, key: &str) -> String {
    let field = if key == "failing" {
        "failing"
    } else {
        "workers"
    };
    let names = worker_pressure
        .get(field)
        .and_then(Value::as_array)
        .map(|workers| {
            workers
                .iter()
                .filter_map(|worker| worker.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if names.is_empty() {
        "unknown".to_owned()
    } else {
        names.join(", ")
    }
}

fn worker_pressure_detail(worker: &Value) -> Value {
    json!({
        "name": string_field(worker, "name"),
        "state": string_field(worker, "state"),
        "health": string_field(worker, "health"),
        "failure_count": worker.get("failure_count").and_then(Value::as_u64).unwrap_or(0),
        "consecutive_failures": worker.get("consecutive_failures").and_then(Value::as_u64).unwrap_or(0),
        "due_count": worker.get("due_count").and_then(Value::as_u64).unwrap_or(0),
        "in_flight_count": worker.get("in_flight_count").and_then(Value::as_u64).unwrap_or(0),
        "oldest_due_age_ms": worker.get("oldest_due_age_ms").cloned().unwrap_or(Value::Null),
        "backoff_or_circuit_open": worker.get("backoff_or_circuit_open").and_then(Value::as_bool).unwrap_or(false)
    })
}

fn worker_is_pressured(worker: &Value) -> bool {
    worker.get("state").and_then(Value::as_str) == Some("started")
        && worker.get("health").and_then(Value::as_str) == Some("pressured")
}

fn worker_is_failing(worker: &Value) -> bool {
    if worker.get("state").and_then(Value::as_str) != Some("started") {
        return true;
    }
    !matches!(
        worker.get("health").and_then(Value::as_str),
        Some("ok" | "pressured")
    )
}

fn witness_run_references(repo_root: &Path) -> Value {
    let mut command = Command::new("gh");
    command
        .current_dir(repo_root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .args([
            "run",
            "list",
            "--workflow",
            WITNESS_WORKFLOW_NAME,
            "--branch",
            WITNESS_WORKFLOW_BRANCH,
            "--limit",
            "3",
            "--json",
            "databaseId,status,conclusion,createdAt,updatedAt,url,event,workflowName",
        ]);

    match command.output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            match serde_json::from_str::<Value>(&stdout) {
                Ok(Value::Array(runs)) => json!({
                    "ok": true,
                    "workflow": WITNESS_WORKFLOW_NAME,
                    "branch": WITNESS_WORKFLOW_BRANCH,
                    "command": WITNESS_RUN_LIST_COMMAND,
                    "artifact_name_pattern": WITNESS_ARTIFACT_PATTERN,
                    "runs": runs.into_iter().map(enrich_witness_run).collect::<Vec<_>>()
                }),
                Ok(value) => json!({
                    "ok": false,
                    "workflow": WITNESS_WORKFLOW_NAME,
                    "command": WITNESS_RUN_LIST_COMMAND,
                    "artifact_name_pattern": WITNESS_ARTIFACT_PATTERN,
                    "reason": "gh returned non-array JSON",
                    "response": value
                }),
                Err(error) => json!({
                    "ok": false,
                    "workflow": WITNESS_WORKFLOW_NAME,
                    "command": WITNESS_RUN_LIST_COMMAND,
                    "artifact_name_pattern": WITNESS_ARTIFACT_PATTERN,
                    "reason": format!("could not parse gh run list JSON: {error}")
                }),
            }
        }
        Ok(output) => json!({
            "ok": false,
            "workflow": WITNESS_WORKFLOW_NAME,
            "branch": WITNESS_WORKFLOW_BRANCH,
            "command": WITNESS_RUN_LIST_COMMAND,
            "artifact_name_pattern": WITNESS_ARTIFACT_PATTERN,
            "reason": redact_text(&String::from_utf8_lossy(&output.stderr))
        }),
        Err(error) => json!({
            "ok": false,
            "workflow": WITNESS_WORKFLOW_NAME,
            "branch": WITNESS_WORKFLOW_BRANCH,
            "command": WITNESS_RUN_LIST_COMMAND,
            "artifact_name_pattern": WITNESS_ARTIFACT_PATTERN,
            "reason": error.to_string()
        }),
    }
}

fn enrich_witness_run(mut run: Value) -> Value {
    if let Some(object) = run.as_object_mut()
        && let Some(id) = object.get("databaseId").and_then(Value::as_u64)
    {
        object.insert(
            "artifact_name".to_owned(),
            Value::String(format!("canary-witness-{id}")),
        );
    }
    run
}

fn verdict_summary(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "missing".to_owned();
    };
    format!(
        "{}; next: {}",
        string_field(value, "overall"),
        string_field(value, "next_operator_action")
    )
}

fn dr_evidence_report(repo_root: &Path) -> Value {
    let status = run_dr_status(repo_root);
    json!({
        "status": status,
        "restore_receipt": latest_restore_receipt(repo_root)
    })
}

fn run_dr_status(repo_root: &Path) -> Value {
    let program = repo_root.join("bin/dr-status");
    if !program.is_file() {
        return json!({
            "ok": false,
            "reason": "bin/dr-status not found"
        });
    }
    match Command::new(&program)
        .current_dir(repo_root)
        .env("NO_COLOR", "1")
        .arg("--app")
        .arg("canary-obs")
        .output()
    {
        Ok(output) => json!({
            "ok": output.status.success(),
            "status": output
                .status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
            "command": "NO_COLOR=1 bin/dr-status --app canary-obs",
            "stdout": redact_text(&String::from_utf8_lossy(&output.stdout)),
            "stderr": redact_text(&String::from_utf8_lossy(&output.stderr))
        }),
        Err(error) => json!({
            "ok": false,
            "reason": error.to_string(),
            "command": "NO_COLOR=1 bin/dr-status --app canary-obs"
        }),
    }
}

fn latest_restore_receipt(repo_root: &Path) -> Value {
    let architecture_dir = repo_root.join("docs/architecture");
    let latest = fs::read_dir(&architecture_dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(std::result::Result::ok))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_restore_receipt_filename)
        })
        .max();
    if let Some(path) = latest {
        return json!({
            "ok": true,
            "path": path.strip_prefix(repo_root).unwrap_or(path.as_path()).display().to_string()
        });
    }
    json!({
        "ok": false,
        "path": "docs/backup-restore-dr.md",
        "reason": "no architecture DR receipt found"
    })
}

fn is_restore_receipt_filename(name: &str) -> bool {
    name.contains("dr-restore")
        || name.contains("restore-drill")
        || name.contains("restore-receipt")
        || name.contains("restore-evidence")
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
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;
    while let Some(index) = remaining.find("sk_") {
        output.push_str(&remaining[..index]);
        output.push_str("sk_...");
        let secret = &remaining[index..];
        let end = secret
            .char_indices()
            .find_map(|(offset, ch)| {
                if offset > 0 && !matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-') {
                    Some(offset)
                } else {
                    None
                }
            })
            .unwrap_or(secret.len());
        remaining = &secret[end..];
    }
    output.push_str(remaining);
    output
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

fn bool_field(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn nullable_string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("none")
        .to_owned()
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
    let url = format!("{}{}", client.endpoint(), path);
    let response = match client.client.get(url).send() {
        Ok(response) => response,
        Err(error) => return json!({"ok": false, "error": error.to_string()}),
    };
    let status = response.status();
    let body = match response.text() {
        Ok(body) => body,
        Err(error) => return json!({"ok": false, "error": error.to_string()}),
    };
    let parsed = serde_json::from_str::<Value>(&body);
    match (status.is_success(), parsed) {
        (true, Ok(value)) => json!({"ok": true, "http_status": status.as_u16(), "response": value}),
        (true, Err(error)) => {
            json!({"ok": false, "http_status": status.as_u16(), "error": error.to_string()})
        }
        (false, Ok(value)) => json!({
            "ok": false,
            "http_status": status.as_u16(),
            "error": format!("GET {path} returned {status}"),
            "response": value
        }),
        (false, Err(_)) => json!({
            "ok": false,
            "http_status": status.as_u16(),
            "error": format!("GET {path} returned {status}: {}", redact_text(&body))
        }),
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

fn worker_readiness_report(readyz_probe: &Value) -> Value {
    let Some(response) = readyz_probe.get("response") else {
        return json!({
            "available": false,
            "reason": "readyz response missing"
        });
    };
    let Some(workers) = response
        .get("checks")
        .and_then(|checks| checks.get("workers"))
        .and_then(Value::as_array)
    else {
        return json!({
            "available": false,
            "reason": "readyz worker checks missing"
        });
    };

    let schema_missing_health_fields = workers
        .iter()
        .filter(|worker| worker.get("health").and_then(Value::as_str).is_none())
        .count();
    let failing_workers = workers
        .iter()
        .filter(|worker| worker_is_failing(worker))
        .count();
    let pressured_workers = workers
        .iter()
        .filter(|worker| worker_is_pressured(worker))
        .count();

    json!({
        "available": true,
        "status": response
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        "worker_count": workers.len(),
        "failing_workers": failing_workers,
        "pressured_workers": pressured_workers,
        "schema_missing_health_fields": schema_missing_health_fields,
        "workers": workers
    })
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

fn worker_readiness_summary(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "missing".to_owned();
    };
    if !value
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return format!("unavailable: {}", string_field(value, "reason"));
    }
    let worker_count = value
        .get("worker_count")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            value
                .get("workers")
                .and_then(Value::as_array)
                .map_or(0, |workers| workers.len() as i64)
        });
    let failing_workers = value
        .get("failing_workers")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            value
                .get("workers")
                .and_then(Value::as_array)
                .map_or(0, |workers| {
                    workers
                        .iter()
                        .filter(|worker| worker_is_failing(worker))
                        .count() as i64
                })
        });
    let pressured_workers = value
        .get("pressured_workers")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            value
                .get("workers")
                .and_then(Value::as_array)
                .map_or(0, |workers| {
                    workers
                        .iter()
                        .filter(|worker| worker_is_pressured(worker))
                        .count() as i64
                })
        });
    let missing_health = value
        .get("schema_missing_health_fields")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut summary = format!(
        "{} {} workers, {} failing",
        string_field(value, "status"),
        worker_count,
        failing_workers
    );
    if pressured_workers > 0 {
        summary.push_str(&format!(", {pressured_workers} pressured"));
    }
    if missing_health > 0 {
        summary.push_str(&format!(", {missing_health} missing health fields"));
    }
    summary
}

fn alert_plane_summary(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "missing".to_owned();
    };
    if !value
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return format!("unavailable: {}", string_field(value, "reason"));
    }
    let status = string_field(value, "status");
    let worker_count = value
        .get("worker_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if status == "healthy" {
        return format!("healthy {worker_count} workers");
    }
    let impaired_workers = value
        .get("impaired_workers")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let noun = if impaired_workers == 1 {
        "worker"
    } else {
        "workers"
    };
    format!(
        "{status} {impaired_workers} {noun}: {}",
        alert_plane_reason_summary(value)
    )
}

fn dr_summary(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "missing".to_owned();
    };
    let status_ok = value
        .get("status")
        .and_then(|status| status.get("ok"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let receipt = value
        .get("restore_receipt")
        .and_then(|receipt| receipt.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let receipt_ok = value
        .get("restore_receipt")
        .and_then(|receipt| receipt.get("ok"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if status_ok {
        if receipt_ok {
            format!("litestream ok, restore_receipt={receipt}")
        } else {
            let reason = value
                .get("restore_receipt")
                .and_then(|receipt| receipt.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("restore evidence missing");
            format!("litestream ok, restore_receipt_missing: {reason}, fallback={receipt}")
        }
    } else {
        let reason = value
            .get("status")
            .and_then(|status| status.get("reason").or_else(|| status.get("stderr")))
            .and_then(Value::as_str)
            .unwrap_or("dr-status failed");
        format!("litestream unavailable: {reason}, restore_receipt={receipt}")
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
            name: "canary_services",
            description: "List target and monitor service states from `bin/canary services`.",
            input_schema: json!({"type":"object","properties":{"state":{"type":"string","enum":["up","degraded","down","unknown"]},"window":{"type":"string","enum":["1h","6h","24h","7d","30d"]}}}),
        },
        ToolSpec {
            name: "canary_incidents",
            description: "Inspect active incidents from `bin/canary incidents`; use this after doctor reports an open Canary incident.",
            input_schema: json!({"type":"object","properties":{"open":{"type":"boolean","default":true}}}),
        },
        ToolSpec {
            name: "canary_timeline",
            description: "Inspect timeline events globally or for one service.",
            input_schema: json!({"type":"object","properties":{"service":{"type":"string"},"window":{"type":"string","enum":["1h","6h","24h","7d","30d"]},"limit":{"type":"integer","minimum":1,"maximum":100}}}),
        },
        ToolSpec {
            name: "canary_targets",
            description: "List configured HTTP uptime targets.",
            input_schema: json!({"type":"object","properties":{}}),
        },
        ToolSpec {
            name: "canary_monitors",
            description: "List configured non-HTTP monitors and check-in watchers.",
            input_schema: json!({"type":"object","properties":{}}),
        },
        ToolSpec {
            name: "canary_doctor",
            description: "Run the agent-oriented Canary doctor check.",
            input_schema: json!({"type":"object","properties":{}}),
        },
        ToolSpec {
            name: "canary_witness",
            description: "Inspect the external `canary-watchman` state and GitHub Actions receipt refs; replacement command: `bin/canary doctor --json | jq '.response.witness,.response.verdict.receipt_run_references'`.",
            input_schema: json!({"type":"object","properties":{}}),
        },
        ToolSpec {
            name: "canary_dr_status",
            description: "Inspect DR/Litestream evidence surfaced by doctor; replacement command: `bin/canary doctor --json | jq '.response.dr'`.",
            input_schema: json!({"type":"object","properties":{}}),
        },
        ToolSpec {
            name: "canary_dogfood_audit",
            description: "Run deployed-service dogfood coverage audit; strict mode exits nonzero after printing JSON gaps.",
            input_schema: json!({"type":"object","properties":{"strict":{"type":"boolean","default":false}}}),
        },
        ToolSpec {
            name: "canary_dogfood_value",
            description: "Build one per-service dogfood value receipt from coverage, health, errors, incidents, claims, annotations, telemetry, and verification evidence.",
            input_schema: json!({"type":"object","required":["service"],"properties":{"service":{"type":"string"},"window":{"type":"string","enum":["1h","6h","24h","7d","30d"]}}}),
        },
        ToolSpec {
            name: "canary_event_capture",
            description: "Capture one bounded analytics event as a telemetry.event timeline row.",
            input_schema: json!({"type":"object","required":["service","name","summary"],"properties":{"service":{"type":"string"},"name":{"type":"string"},"summary":{"type":"string"},"severity":{"type":"string","enum":["info","warning","error"]},"attributes":{"type":"object"},"retention_class":{"type":"string","enum":["ephemeral","standard","audit"]},"privacy_policy":{"type":"string","enum":["redacted","public","sensitive"]},"sampling_policy":{"type":"string"}}}),
        },
        ToolSpec {
            name: "canary_claims_list",
            description: "List remediation claims for one Canary subject.",
            input_schema: json!({"type":"object","required":["subject_type","subject_id"],"properties":{"subject_type":{"type":"string","enum":["incident","error_group","target","monitor"]},"subject_id":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50},"cursor":{"type":"string"}}}),
        },
        ToolSpec {
            name: "canary_claim_get",
            description: "Read one remediation claim by id.",
            input_schema: json!({"type":"object","required":["claim_id"],"properties":{"claim_id":{"type":"string"}}}),
        },
        ToolSpec {
            name: "canary_claim_create",
            description: "Claim one Canary subject for agentic remediation with owner, purpose, TTL, and idempotency key.",
            input_schema: json!({"type":"object","required":["subject_type","subject_id","owner","purpose","ttl_ms","idempotency_key"],"properties":{"subject_type":{"type":"string","enum":["incident","error_group","target","monitor"]},"subject_id":{"type":"string"},"owner":{"type":"string"},"purpose":{"type":"string"},"idempotency_key":{"type":"string"},"ttl_ms":{"type":"integer","minimum":1},"evidence_links":{"type":"array","items":{"type":"string"}}}}),
        },
        ToolSpec {
            name: "canary_claim_transition",
            description: "Transition one remediation claim to a bounded state.",
            input_schema: json!({"type":"object","required":["claim_id","owner","state"],"properties":{"claim_id":{"type":"string"},"owner":{"type":"string"},"state":{"type":"string","enum":["claimed","investigating","fix_proposed","verified","dismissed","expired","released"]},"evidence_links":{"type":"array","items":{"type":"string"}}}}),
        },
        ToolSpec {
            name: "canary_claim_release",
            description: "Release one remediation claim.",
            input_schema: json!({"type":"object","required":["claim_id","owner"],"properties":{"claim_id":{"type":"string"},"owner":{"type":"string"}}}),
        },
        ToolSpec {
            name: "canary_integrate_discover",
            description: "Discover local project integration state without reading secret values.",
            input_schema: json!({"type":"object","required":["path_or_project"],"properties":{"path_or_project":{"type":"string"},"service":{"type":"string"},"production_url":{"type":"string"},"platform_project":{"type":"string"}}}),
        },
        ToolSpec {
            name: "canary_integrate_status",
            description: "Merge local scan, integration receipt, live Canary state, query readback, webhooks, and dogfood evidence into one coverage verdict.",
            input_schema: json!({"type":"object","required":["path_or_project"],"properties":{"path_or_project":{"type":"string"},"service":{"type":"string"},"production_url":{"type":"string"},"platform_project":{"type":"string"}}}),
        },
        ToolSpec {
            name: "canary_integrate_plan",
            description: "Emit a reviewable Canary integration patch and enrollment plan.",
            input_schema: json!({"type":"object","required":["path_or_project"],"properties":{"path_or_project":{"type":"string"},"service":{"type":"string"},"production_url":{"type":"string"},"platform_project":{"type":"string"}}}),
        },
        ToolSpec {
            name: "canary_integrate_patch",
            description: "Apply safe Next.js Canary integration patches after reviewing the plan.",
            input_schema: json!({"type":"object","required":["path_or_project"],"properties":{"path_or_project":{"type":"string"},"service":{"type":"string"},"production_url":{"type":"string"},"platform_project":{"type":"string"}}}),
        },
        ToolSpec {
            name: "canary_integrate_enroll",
            description: "Create a Canary health target and scoped ingest key for a deployed service; redacts the one-time key by default.",
            input_schema: json!({"type":"object","required":["service","url"],"properties":{"service":{"type":"string"},"url":{"type":"string"},"environment":{"type":"string"},"interval_ms":{"type":"integer"},"project_root":{"type":"string"},"show_secret":{"type":"boolean","default":false}}}),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn doctor_verdict_degrades_for_stale_down_watchman_even_when_routes_ready() {
        let healthz = json!({"ok": true, "response": {"status": "ok"}});
        let readyz = json!({"ok": true, "response": {"status": "ready"}});
        let summary = json!({"ok": true, "summary": ["summary: Canary healthy"]});
        let incidents = json!({
            "ok": true,
            "response": {
                "incidents": [{
                    "id": "INC-witness",
                    "service": "canary",
                    "state": "open",
                    "title": "Canary witness failed"
                }]
            }
        });
        let canary_errors = json!({
            "ok": true,
            "response": {
                "service": "canary",
                "total_errors": 0
            }
        });
        let witness = json!({
            "status": "observed",
            "monitor": "canary-watchman",
            "state": "down",
            "last_check_in_status": "alive",
            "last_check_in_at": "2026-06-15T22:00:00Z",
            "expected_every_ms": 600000
        });
        let worker_readiness = json!({
            "available": true,
            "status": "ready",
            "worker_count": 5,
            "failing_workers": 0,
            "pressured_workers": 0,
            "workers": []
        });
        let alert_plane = alert_plane_report(&worker_readiness);
        let dogfood = json!({
            "ok": true,
            "response": {
                "strict_failures": [
                    {"kind": "missing_readback", "service": "vanity"},
                    {"kind": "missing_target", "service": "linejam"}
                ]
            }
        });
        let receipt_runs = json!({
            "ok": true,
            "workflow": "Canary Witness",
            "runs": [{
                "databaseId": 123456,
                "status": "completed",
                "conclusion": "failure",
                "url": "https://github.com/example/canary/actions/runs/123456",
                "artifact_name": "canary-witness-123456"
            }]
        });

        let verdict = doctor_verdict(
            &healthz,
            &readyz,
            &summary,
            &incidents,
            &canary_errors,
            &witness,
            &worker_readiness,
            &alert_plane,
            &dogfood,
            &receipt_runs,
            1_781_561_520_000,
        );

        assert_eq!(verdict["overall"], "degraded");
        assert_eq!(verdict["witness_age_ms"], 720000);
        assert_eq!(verdict["open_canary_incident"]["id"], "INC-witness");
        assert_eq!(verdict["dogfood_gap_count"], 2);
        assert_eq!(
            verdict["receipt_run_references"]["runs"][0]["artifact_name"],
            "canary-witness-123456"
        );
        assert!(
            verdict["blocking_signals"]
                .as_array()
                .is_some_and(|signals| signals.iter().any(|signal| signal
                    .as_str()
                    .unwrap_or_default()
                    .contains("canary-watchman down")))
        );
        assert!(
            verdict["next_operator_action"]
                .as_str()
                .unwrap_or_default()
                .contains("gh workflow run")
        );
    }

    #[test]
    fn doctor_verdict_degrades_for_alert_plane_pressure_even_when_readyz_is_ready() {
        let healthz = json!({"ok": true, "response": {"status": "ok"}});
        let readyz = json!({"ok": true, "response": {"status": "ready"}});
        let summary = json!({"ok": true, "summary": ["summary: Canary healthy"]});
        let incidents = json!({"ok": true, "response": {"incidents": []}});
        let canary_errors = json!({
            "ok": true,
            "response": {
                "service": "canary",
                "total_errors": 0
            }
        });
        let witness = json!({
            "status": "observed",
            "monitor": "canary-watchman",
            "state": "up",
            "last_check_in_status": "alive",
            "last_check_in_at": "2026-06-15T22:00:00Z",
            "expected_every_ms": 600000
        });
        let worker_readiness = json!({
            "available": true,
            "status": "ready",
            "worker_count": 2,
            "failing_workers": 0,
            "pressured_workers": 1,
            "workers": [
                {"name": "monitor_overdue", "state": "started", "health": "pressured", "failure_count": 0, "consecutive_failures": 0, "due_count": 1, "oldest_due_age_ms": 7200000, "backoff_or_circuit_open": false},
                {"name": "target_probe", "state": "started", "health": "ok", "failure_count": 0, "consecutive_failures": 0, "due_count": 0, "oldest_due_age_ms": null, "backoff_or_circuit_open": false}
            ]
        });
        let alert_plane = alert_plane_report(&worker_readiness);
        let dogfood = json!({"ok": true, "response": {"strict_failures": []}});
        let receipt_runs = json!({"ok": true, "runs": []});

        let verdict = doctor_verdict(
            &healthz,
            &readyz,
            &summary,
            &incidents,
            &canary_errors,
            &witness,
            &worker_readiness,
            &alert_plane,
            &dogfood,
            &receipt_runs,
            1_781_561_520_000,
        );

        assert_eq!(alert_plane["status"], "impaired");
        assert_eq!(verdict["overall"], "degraded");
        assert_eq!(verdict["alert_plane"]["status"], "impaired");
        assert!(
            verdict["blocking_signals"]
                .as_array()
                .is_some_and(|signals| signals.iter().any(|signal| signal
                    .as_str()
                    .unwrap_or_default()
                    .contains("alert-plane impaired: monitor_overdue pressured")))
        );
        assert!(
            verdict["next_operator_action"]
                .as_str()
                .unwrap_or_default()
                .contains("alert-plane")
        );
    }

    #[test]
    fn worker_readiness_keeps_pressured_workers_separate_from_failures() {
        let readyz = json!({
            "ok": true,
            "response": {
                "status": "ready",
                "checks": {
                    "workers": [
                        {"name": "webhook_delivery", "state": "started", "health": "pressured", "failure_count": 0, "due_count": 5, "oldest_due_age_ms": 120000},
                        {"name": "target_probe", "state": "started", "health": "ok", "failure_count": 0}
                    ]
                }
            }
        });

        let report = worker_readiness_report(&readyz);

        assert_eq!(report["status"], "ready");
        assert_eq!(report["worker_count"], 2);
        assert_eq!(report["failing_workers"], 0);
        assert_eq!(report["pressured_workers"], 1);
        let alert_plane = alert_plane_report(&report);
        assert_eq!(alert_plane["status"], "impaired");
        assert_eq!(
            alert_plane_summary(Some(&alert_plane)),
            "impaired 1 worker: webhook_delivery pressured"
        );
        assert_eq!(
            worker_readiness_summary(Some(&report)),
            "ready 2 workers, 0 failing, 1 pressured"
        );
    }

    #[test]
    fn alert_plane_uses_not_ready_worker_snapshots() {
        let readyz = json!({
            "ok": false,
            "http_status": 503,
            "error": "GET /readyz returned 503 Service Unavailable",
            "response": {
                "status": "not_ready",
                "checks": {
                    "workers": [
                        {"name": "webhook_delivery", "state": "started", "health": "pressured", "failure_count": 0, "consecutive_failures": 0, "due_count": 9, "oldest_due_age_ms": 7200000, "backoff_or_circuit_open": true},
                        {"name": "target_probe", "state": "started", "health": "failing", "failure_count": 3, "consecutive_failures": 3, "due_count": 1, "oldest_due_age_ms": 0, "backoff_or_circuit_open": false}
                    ]
                }
            }
        });

        let report = worker_readiness_report(&readyz);
        let alert_plane = alert_plane_report(&report);

        assert_eq!(report["available"], true);
        assert_eq!(report["status"], "not_ready");
        assert_eq!(report["failing_workers"], 1);
        assert_eq!(report["pressured_workers"], 1);
        assert_eq!(alert_plane["status"], "impaired");
        assert_eq!(
            alert_plane["reasons"],
            json!([
                "webhook_delivery backoff_or_circuit_open",
                "target_probe failing"
            ])
        );
    }

    #[test]
    fn ingest_config_prefers_ingest_key_over_read_key()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("ingest-config")?;
        let config_path = root.join("config.json");
        fs::write(
            &config_path,
            r#"{"endpoint":"https://canary.example","read_api_key":"sk_read","ingest_api_key":"sk_ingest","admin_api_key":"sk_admin"}"#,
        )?;

        let read_config = Config::resolve(None, None, Some(config_path.clone()))?;
        let ingest_config = Config::resolve_for_ingest(None, None, Some(config_path))?;

        assert_eq!(read_config.api_key()?, "sk_admin");
        assert_eq!(ingest_config.api_key()?, "sk_ingest");
        assert_eq!(
            ingest_config.redacted_key(),
            "config.ingest_api_key: redacted"
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn restore_receipt_discovery_requires_restore_specific_receipts()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("restore-receipt")?;
        let architecture = root.join("docs/architecture");
        fs::create_dir_all(&architecture)?;
        fs::write(
            architecture.join("rust-write-path-evidence-2026-06-12.md"),
            "write path evidence, not a restore drill",
        )?;

        let missing = latest_restore_receipt(&root);
        assert_eq!(missing["ok"], false);
        assert_eq!(missing["path"], "docs/backup-restore-dr.md");

        fs::write(
            architecture.join("restore-drill-evidence-2026-06-14.md"),
            "restore drill evidence",
        )?;
        let found = latest_restore_receipt(&root);
        assert_eq!(found["ok"], true);
        assert_eq!(
            found["path"],
            "docs/architecture/restore-drill-evidence-2026-06-14.md"
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn integration_discovery_reports_secret_names_without_values()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("discover")?;
        fs::create_dir_all(root.join(".vercel"))?;
        fs::write(
            root.join(".vercel/project.json"),
            r#"{"projectName":"Misty Step"}"#,
        )?;
        fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"next":"15.0.0","@sentry/nextjs":"8.0.0"}}"#,
        )?;
        fs::write(
            root.join(".env.local"),
            "CANARY_API_KEY=sk_live_should_not_leak\nSENTRY_DSN=https://secret@example.com\n",
        )?;
        fs::write(
            root.join("instrumentation.ts"),
            "import * as Sentry from '@sentry/nextjs';\n",
        )?;

        let discovery = integration_discover(&IntegrationInput {
            target: root.clone(),
            service: None,
            production_url: Some("https://www.mistystep.io".to_owned()),
            platform_project: None,
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        })?;
        let rendered = serde_json::to_string(&discovery)?;

        assert_eq!(discovery["framework"], "nextjs");
        assert_eq!(discovery["platform"], "vercel");
        assert_eq!(discovery["service"], "misty step");
        assert_eq!(
            discovery["signals"]["canary_env_names"],
            json!(["CANARY_API_KEY"])
        );
        assert_eq!(
            discovery["signals"]["sentry_env_names"],
            json!(["SENTRY_DSN"])
        );
        assert!(
            discovery["signals"]["sentry_present"]
                .as_bool()
                .unwrap_or(false)
        );
        assert!(
            !discovery["signals"]["canary_present"]
                .as_bool()
                .unwrap_or(true)
        );
        assert_eq!(discovery["signals"]["canary_code_paths"], json!([]));
        assert!(!rendered.contains("sk_live_should_not_leak"));
        assert!(!rendered.contains("secret@example.com"));

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn integration_plan_distinguishes_patch_platform_and_enrollment_actions()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("plan")?;
        fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"next":"15.0.0"}}"#,
        )?;

        let plan = integration_plan(&IntegrationInput {
            target: root.clone(),
            service: Some("timeismoney-splash".to_owned()),
            production_url: Some("https://www.timeismoney.works".to_owned()),
            platform_project: None,
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        })?;
        let actions = plan["actions"].as_array().ok_or("missing actions")?;

        assert_eq!(plan["can_patch"], true);
        assert!(has_action(actions, "sdk_dependency", "needed"));
        assert!(has_action(actions, "server_instrumentation", "needed"));
        assert!(has_action(actions, "health_route", "needed"));
        assert!(has_action(actions, "global_error_capture", "needed"));
        assert!(has_action(actions, "env_names", "needed"));
        assert!(has_action(actions, "target_enrollment", "needed"));
        assert!(has_action(actions, "integration_receipt", "needed"));
        assert_eq!(
            action(actions, "target_enrollment")?["health_url"],
            "https://www.timeismoney.works/api/health"
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn integration_plan_distinguishes_server_and_browser_canary_capture()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("roles")?;
        fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"next":"15.0.0","@canary-obs/sdk":"0.1.0"}}"#,
        )?;
        fs::create_dir_all(root.join("app"))?;
        fs::write(
            root.join("app/global-error.tsx"),
            "import { captureException } from '@canary-obs/sdk';\n",
        )?;

        let plan = integration_plan(&IntegrationInput {
            target: root.clone(),
            service: Some("browser-only".to_owned()),
            production_url: Some("https://example.com".to_owned()),
            platform_project: None,
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        })?;
        let actions = plan["actions"].as_array().ok_or("missing actions")?;

        assert!(has_action(actions, "server_instrumentation", "needed"));
        assert!(has_action(actions, "global_error_capture", "present"));

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn integration_discovery_finds_src_app_route_groups_and_bespoke_canary_code()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("grouped-health")?;
        fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"next":"15.0.0"}}"#,
        )?;
        fs::create_dir_all(root.join("src/app/(app)/api/health"))?;
        fs::write(
            root.join("src/app/(app)/api/health/route.ts"),
            "export function GET() { return Response.json({ status: 'ok' }); }\n",
        )?;
        fs::create_dir_all(root.join("src/lib"))?;
        fs::write(
            root.join("src/lib/canary-reporter.ts"),
            "export const endpoint = process.env.CANARY_ENDPOINT;\nfetch(`${endpoint}/api/v1/errors`);\n",
        )?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("src/lib/loop"))?;

        let discovery = integration_discover(&IntegrationInput {
            target: root.clone(),
            service: Some("chrondle".to_owned()),
            production_url: Some("https://www.chrondle.app".to_owned()),
            platform_project: None,
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        })?;

        assert_eq!(
            discovery["health_routes"],
            json!([{"path": "src/app/(app)/api/health/route.ts", "route": "/api/health"}])
        );
        assert_eq!(discovery["signals"]["canary_present"], true);
        assert_eq!(
            discovery["signals"]["canary_code_paths"],
            json!([{"kind": "canary", "path": "src/lib/canary-reporter.ts"}])
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn integration_discovery_merges_receipt_env_names()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("receipt-env")?;
        fs::create_dir_all(root.join(".canary"))?;
        fs::write(
            root.join(INTEGRATION_RECEIPT_PATH),
            r#"{"schema_version":1,"service":"vanity","last_verified_at":"1781380800","env_names":["CANARY_ENDPOINT","CANARY_API_KEY"]}"#,
        )?;

        let discovery = integration_discover(&IntegrationInput {
            target: root.clone(),
            service: Some("vanity".to_owned()),
            production_url: None,
            platform_project: None,
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        })?;

        assert_eq!(
            discovery["signals"]["declared_env_names"],
            json!(["CANARY_API_KEY", "CANARY_ENDPOINT"])
        );
        assert_eq!(discovery["signals"]["platform_env_names"], json!([]));
        assert_eq!(
            discovery["signals"]["canary_env_names"],
            json!(["CANARY_API_KEY", "CANARY_ENDPOINT"])
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn vercel_env_parser_extracts_names_without_values()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let stdout = r#"Retrieving project...
{
  "envs": [
    {"key": "CANARY_ENDPOINT", "type": "encrypted"},
    {"key": "CANARY_API_KEY", "type": "encrypted"},
    {"key": "lowercase_ignored", "type": "plain"}
  ]
}
Vercel CLI completed
"#;

        let names = vercel_env_names_from_stdout(stdout)?;

        assert_eq!(names, vec!["CANARY_API_KEY", "CANARY_ENDPOINT"]);
        Ok(())
    }

    #[test]
    fn integration_plan_uses_static_site_health_target_without_code_patch()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("static")?;
        fs::write(root.join("index.html"), "<h1>static</h1>")?;

        let plan = integration_plan(&IntegrationInput {
            target: root.clone(),
            service: Some("trump-goggles-splash".to_owned()),
            production_url: Some("https://trumpgoggles.com".to_owned()),
            platform_project: Some("trump-goggles".to_owned()),
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        })?;
        let actions = plan["actions"].as_array().ok_or("missing actions")?;

        assert_eq!(plan["framework"], "unknown");
        assert_eq!(plan["coverage_mode"], "static");
        assert_eq!(plan["can_patch"], false);
        assert_eq!(
            action(actions, "target_enrollment")?["health_url"],
            "https://trumpgoggles.com"
        );
        assert_eq!(
            plan["static_site"]["target_url"],
            "https://trumpgoggles.com"
        );
        assert!(
            plan["static_site"]["browser_capture_snippet"]
                .as_str()
                .unwrap_or_default()
                .contains("NEXT_PUBLIC_CANARY_API_KEY")
        );
        assert!(
            plan["static_site"]["browser_capture_snippet"]
                .as_str()
                .unwrap_or_default()
                .contains("@canary-obs/sdk/nextjs")
        );
        assert!(
            plan["static_site"]["browser_capture_warning"]
                .as_str()
                .unwrap_or_default()
                .contains("never expose admin")
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn integration_plan_includes_non_http_monitor_templates()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("monitors")?;
        fs::write(root.join("package.json"), r#"{"dependencies":{}}"#)?;

        let plan = integration_plan(&IntegrationInput {
            target: root.clone(),
            service: Some("worker-app".to_owned()),
            production_url: None,
            platform_project: None,
            endpoint: "https://canary.example".to_owned(),
        })?;
        let templates = plan["monitor_templates"]
            .as_array()
            .ok_or("missing monitor templates")?;

        assert_eq!(templates.len(), 4);
        assert!(templates.iter().any(|template| {
            template["runtime"] == "cron"
                && template["mode"] == "schedule"
                && template["check_in"]
                    .as_str()
                    .is_some_and(|command| command.contains("/api/v1/check-ins"))
        }));
        assert!(
            templates
                .iter()
                .any(|template| { template["runtime"] == "desktop" && template["mode"] == "ttl" })
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn integration_patch_adds_nextjs_files_and_refuses_foreign_overwrites()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("patch")?;
        fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"next":"15.0.0"}}"#,
        )?;
        fs::write(
            root.join("instrumentation.ts"),
            "export function register() {}\n",
        )?;

        let patched = integration_patch(&IntegrationInput {
            target: root.clone(),
            service: Some("vanity".to_owned()),
            production_url: Some("https://www.phaedrus.io".to_owned()),
            platform_project: None,
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        })?;
        let changes = patched["changes"].as_array().ok_or("missing changes")?;
        let package = fs::read_to_string(root.join("package.json"))?;
        let instrumentation = fs::read_to_string(root.join("instrumentation.ts"))?;
        let health = fs::read_to_string(root.join("app/api/health/route.ts"))?;
        let global_error = fs::read_to_string(root.join("app/global-error.tsx"))?;
        let receipt = read_json_file(&root.join(INTEGRATION_RECEIPT_PATH))?;

        assert!(package.contains("@canary-obs/sdk"));
        assert_eq!(instrumentation, "export function register() {}\n");
        assert!(health.contains("status: \"ok\""));
        assert!(
            global_error.contains("service: process.env.NEXT_PUBLIC_CANARY_SERVICE ?? \"vanity\"")
        );
        assert!(changes.iter().any(|change| {
            change["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("instrumentation.ts"))
                && change["status"] == "skipped"
        }));
        assert!(changes.iter().any(|change| {
            change["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(INTEGRATION_RECEIPT_PATH))
                && change["status"] == "updated"
        }));
        assert_eq!(receipt["service"], "vanity");
        assert_eq!(receipt["health_url"], "https://www.phaedrus.io/api/health");
        assert_eq!(receipt["verification_status"], "planned");
        assert_eq!(receipt["target_id"], Value::Null);
        assert_eq!(
            receipt["env_names"],
            json!([
                "CANARY_ENDPOINT",
                "CANARY_API_KEY",
                "NEXT_PUBLIC_CANARY_ENDPOINT",
                "NEXT_PUBLIC_CANARY_API_KEY"
            ])
        );

        write_integration_receipt(
            &root,
            &json!({
                "schema_version": 1,
                "service": "vanity",
                "environment": "production",
                "canary_endpoint": DEFAULT_ENDPOINT,
                "health_url": "https://www.phaedrus.io/api/health",
                "target_id": "TGT-existing",
                "monitor_ids": ["MON-existing"],
                "webhook_ids": ["WHK-existing"],
                "api_key_id": "KEY-existing",
                "verification_status": "verified",
                "env_names": ["CANARY_ENDPOINT", "CANARY_API_KEY"],
                "verification_commands": [],
                "last_verified_at": "1781380800"
            }),
        )?;
        integration_patch(&IntegrationInput {
            target: root.clone(),
            service: Some("vanity".to_owned()),
            production_url: Some("https://www.phaedrus.io".to_owned()),
            platform_project: None,
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        })?;
        let preserved_receipt = read_json_file(&root.join(INTEGRATION_RECEIPT_PATH))?;
        assert_eq!(preserved_receipt["verification_status"], "verified");
        assert_eq!(preserved_receipt["target_id"], "TGT-existing");
        assert_eq!(preserved_receipt["monitor_ids"], json!(["MON-existing"]));
        assert_eq!(preserved_receipt["webhook_ids"], json!(["WHK-existing"]));
        assert_eq!(preserved_receipt["api_key_id"], "KEY-existing");
        assert_eq!(preserved_receipt["last_verified_at"], "1781380800");

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn integration_patch_skips_foreign_health_routes_and_escapes_literals()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = temp_project("patch-safe")?;
        fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"next":"15.0.0"}}"#,
        )?;
        fs::create_dir_all(root.join("app/api/health"))?;
        fs::write(
            root.join("app/api/health/route.ts"),
            "export function GET() {\n  return Response.json({ status: \"ok\", database: \"checked\" });\n}\n",
        )?;

        let patched = integration_patch(&IntegrationInput {
            target: root.clone(),
            service: Some("bad\"; throw new Error(\"boom\") //".to_owned()),
            production_url: Some("https://example.com".to_owned()),
            platform_project: None,
            endpoint: "https://canary.example/\"quoted\"".to_owned(),
        })?;
        let changes = patched["changes"].as_array().ok_or("missing changes")?;
        let health = fs::read_to_string(root.join("app/api/health/route.ts"))?;
        let instrumentation = fs::read_to_string(root.join("instrumentation.ts"))?;

        assert!(health.contains("database: \"checked\""));
        assert!(changes.iter().any(|change| {
            change["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("app/api/health/route.ts"))
                && change["status"] == "skipped"
        }));
        assert!(instrumentation.contains(r#"service: "bad\"; throw new Error(\"boom\") //""#));
        assert!(instrumentation.contains(
            r#"endpoint: process.env.CANARY_ENDPOINT ?? "https://canary.example/\"quoted\"""#
        ));

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn integration_enroll_receipt_redaction_removes_one_time_keys() {
        let response = json!({
            "service": "vanity",
            "api_key": {
                "key": "sk_live_secret",
                "warning": "Store this key securely. It will not be shown again."
            },
            "target": {"id": "TGT-1"},
            "nested": {"future": "prefix sk_live_secret suffix"},
            "snippets": {
                "typescript_init": "initCanary({ apiKey: \"sk_live_secret\" })",
                "error_ingest_curl": "Authorization: Bearer sk_live_secret"
            }
        });

        let redacted = redact_secret_value(response);
        let rendered = serde_json::to_string(&redacted).unwrap_or_default();

        assert_eq!(redacted["api_key"]["key"], "redacted");
        assert!(
            redacted["api_key"]["warning"]
                .as_str()
                .unwrap_or_default()
                .contains("redacted")
        );
        assert_eq!(redacted["nested"]["future"], "prefix sk_... suffix");
        assert!(!rendered.contains("sk_live_secret"));
        assert!(rendered.contains("sk_..."));
    }

    #[test]
    fn integration_coverage_verdict_merges_local_live_query_and_dogfood_evidence() {
        let discovery = json!({
            "signals": {
                "canary_present": true,
                "canary_sdk_dependency": false
            }
        });
        let targets = json!({"ok": true, "response": {"targets": [{"service": "linejam"}]}});
        let monitors = json!({"ok": true, "response": {"monitors": []}});
        let webhooks = json!({"ok": true, "response": {"webhooks": [{"id": "WHK-1"}]}});
        let query = json!({"ok": true, "response": {"service": "linejam", "total_errors": 0}});
        let dogfood = json!({"ok": true, "response": {"services": [{"service": "linejam", "state": "active"}]}});

        let verdict = integration_coverage_verdict(
            "linejam", &discovery, &targets, &monitors, &webhooks, &query, &dogfood,
        );

        assert_eq!(verdict["status"], "covered");
        assert_eq!(verdict["target_enrolled"], true);
        assert_eq!(verdict["webhook_configured"], true);
        assert_eq!(verdict["query_readback"], true);
        assert_eq!(verdict["dogfood_state"], "active");
        assert_eq!(verdict["strict_failures"], json!([]));
    }

    #[test]
    fn status_plan_reconciliation_marks_live_enrollment_present() {
        let plan = json!({
            "actions": [
                {
                    "kind": "target_enrollment",
                    "status": "needed",
                    "executor": "enroll"
                }
            ]
        });
        let target = json!({"id": "TGT-1", "service": "linejam"});

        let reconciled = reconcile_plan_with_live_state(&plan, Some(&target), None);

        assert_eq!(reconciled["actions"][0]["status"], "present");
        assert_eq!(reconciled["actions"][0]["executor"], "none");
        assert_eq!(
            reconciled["actions"][0]["live_evidence"]["target_id"],
            "TGT-1"
        );
    }

    #[test]
    fn integration_coverage_does_not_count_null_receipt_as_local_capture() {
        let discovery = json!({
            "integration_receipt": null,
            "signals": {
                "canary_present": false,
                "canary_sdk_dependency": false
            }
        });
        let targets = json!({"ok": true, "response": {"targets": []}});
        let monitors = json!({"ok": true, "response": {"monitors": []}});
        let webhooks = json!({"ok": true, "response": {"webhooks": []}});
        let query = json!({"ok": true, "response": {"service": "linejam", "total_errors": 0}});
        let dogfood = json!({"ok": true, "response": {"registry": []}});

        let verdict = integration_coverage_verdict(
            "linejam", &discovery, &targets, &monitors, &webhooks, &query, &dogfood,
        );

        assert_eq!(verdict["status"], "missing");
        assert_eq!(verdict["local_capture"], false);
    }

    #[test]
    fn dogfood_state_reads_registry_and_surface_inventory_shapes() {
        let registry = json!({"ok": true, "response": {"registry": [
            {"service": "chrondle", "state": "active"}
        ]}});
        let surfaces = json!({"ok": true, "response": {"surfaces": [
            {"service": "misty-step", "coverage": "covered"}
        ]}});

        assert_eq!(dogfood_service_state(&registry, "chrondle"), "active");
        assert_eq!(dogfood_service_state(&surfaces, "misty-step"), "covered");
    }

    #[test]
    fn dogfood_value_receipt_marks_reference_service_proven() {
        let dogfood = json!({"ok": true, "response": {"surfaces": [
            {
                "service": "linejam",
                "coverage": "covered",
                "registry_state": "active",
                "evidence_stale": true,
                "failure_mode": "No current blocker; error budget is healthy; this is the strongest reference integration with Vercel health and Fly responder coverage.",
                "next_action": "Keep as the reference service for webhook/responder coverage.",
                "receipt_seen": false
            }
        ]}});
        let targets = json!({"ok": true, "response": {"targets": [
            {"id": "TGT-linejam", "service": "linejam", "url": "https://linejam.example/health", "active": true}
        ]}});
        let monitors = json!({"ok": true, "response": {"monitors": []}});
        let status = json!({"ok": true, "response": {"targets": [
            {"id": "TGT-linejam", "name": "linejam", "url": "https://linejam.example/health", "state": "up"}
        ], "monitors": []}});
        let query = json!({"ok": true, "response": {"service": "linejam", "window": "24h", "total_errors": 0, "groups": []}});
        let incidents = json!({"ok": true, "response": {"incidents": [
            {"id": "INC-other", "service": "other", "state": "open"}
        ]}});
        let timeline = json!({"ok": true, "response": {"events": [
            {"id": "EVT-telemetry", "event": "telemetry.event", "service": "linejam"},
            {"id": "EVT-error", "event": "error.ingested", "service": "linejam"}
        ]}});
        let claims = json!([
            {"subject_type": "target", "subject_id": "TGT-linejam", "probe": {"ok": true, "response": {"claims": [], "current_claim": null}}}
        ]);
        let annotations = json!([
            {"subject_type": "target", "subject_id": "TGT-linejam", "probe": {"ok": true, "response": {"annotations": []}}}
        ]);

        let receipt = dogfood_value_receipt(&DogfoodValueInput {
            service: "linejam",
            window: "24h",
            dogfood: &dogfood,
            targets: &targets,
            monitors: &monitors,
            status: &status,
            query: &query,
            incidents: &incidents,
            timeline: &timeline,
            claims: &claims,
            annotations: &annotations,
        });

        assert_eq!(receipt["service"], "linejam");
        assert_eq!(receipt["value_state"], "proven");
        assert_eq!(receipt["coverage"]["verdict"], "covered");
        assert_eq!(receipt["registry"]["state"], "active");
        assert_eq!(receipt["health"]["state"], "up");
        assert_eq!(receipt["error_counts"]["total"], 0);
        assert_eq!(receipt["incident_counts"]["open"], 0);
        assert!(receipt["active_remediation_claim"].is_null());
        assert_eq!(
            receipt["recent_annotations"].as_array().map(Vec::len),
            Some(0)
        );
        assert_eq!(
            receipt["recent_telemetry_events"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(receipt["verification"]["status"], "verified");
        assert_eq!(
            receipt["next_action"],
            "Keep as the reference service for webhook/responder coverage."
        );
    }

    #[test]
    fn dogfood_value_receipt_requires_live_status_up_for_proven() {
        let dogfood = json!({"ok": true, "response": {"surfaces": [
            {
                "service": "linejam",
                "coverage": "covered",
                "registry_state": "active",
                "failure_mode": "Current target is under active watch.",
                "next_action": "Keep enrolled.",
                "receipt_seen": true
            }
        ]}});
        let targets = json!({"ok": true, "response": {"targets": [
            {"id": "TGT-linejam", "service": "linejam", "url": "https://linejam.example/health", "active": true}
        ]}});
        let monitors = json!({"ok": true, "response": {"monitors": []}});
        let status = json!({"ok": true, "response": {"targets": [
            {"id": "TGT-linejam", "name": "linejam", "url": "https://linejam.example/health", "state": "down"}
        ], "monitors": []}});
        let query = json!({"ok": true, "response": {"service": "linejam", "window": "24h", "total_errors": 0, "groups": []}});
        let incidents = json!({"ok": true, "response": {"incidents": []}});
        let timeline = json!({"ok": true, "response": {"events": []}});
        let claims = json!([]);
        let annotations = json!([]);

        let receipt = dogfood_value_receipt(&DogfoodValueInput {
            service: "linejam",
            window: "24h",
            dogfood: &dogfood,
            targets: &targets,
            monitors: &monitors,
            status: &status,
            query: &query,
            incidents: &incidents,
            timeline: &timeline,
            claims: &claims,
            annotations: &annotations,
        });

        assert_eq!(receipt["health"]["state"], "down");
        assert_eq!(receipt["value_state"], "partial");
        assert_eq!(receipt["verification"]["status"], "health_not_up");
        assert_eq!(
            receipt["next_action"],
            "Restore live health state: /api/v1/status reports down; then rerun the value receipt."
        );
    }

    #[test]
    fn dogfood_value_receipt_distinguishes_stale_work_from_current_readback() {
        let dogfood = json!({"ok": true, "response": {"surfaces": [
            {
                "service": "chrondle",
                "coverage": "covered",
                "registry_state": "active",
                "evidence_stale": true,
                "failure_mode": "Canary target is up, but the live 24h audit showed a high-volume TypeError group.",
                "next_action": "Keep enrolled and triage the TypeError flood through Canary query evidence.",
                "receipt_seen": false
            }
        ]}});
        let targets = json!({"ok": true, "response": {"targets": [
            {"id": "TGT-chrondle", "service": "chrondle", "url": "https://chrondle.example/health", "active": true}
        ]}});
        let empty = json!({"ok": true, "response": {"monitors": []}});
        let status = json!({"ok": true, "response": {"targets": [
            {"id": "TGT-chrondle", "name": "chrondle", "url": "https://chrondle.example/health", "state": "up"}
        ], "monitors": []}});
        let query = json!({"ok": true, "response": {"service": "chrondle", "window": "24h", "total_errors": 0, "groups": []}});
        let incidents = json!({"ok": true, "response": {"incidents": []}});
        let timeline = json!({"ok": true, "response": {"events": []}});
        let claims = json!([]);
        let annotations = json!([]);

        let receipt = dogfood_value_receipt(&DogfoodValueInput {
            service: "chrondle",
            window: "24h",
            dogfood: &dogfood,
            targets: &targets,
            monitors: &empty,
            status: &status,
            query: &query,
            incidents: &incidents,
            timeline: &timeline,
            claims: &claims,
            annotations: &annotations,
        });

        assert_eq!(receipt["value_state"], "stale_registry_evidence");
        assert_eq!(receipt["error_counts"]["total"], 0);
        assert_eq!(
            receipt["registry"]["failure_mode"],
            "Canary target is up, but the live 24h audit showed a high-volume TypeError group."
        );
        assert_eq!(receipt["verification"]["status"], "needs_evidence_refresh");
        assert!(
            receipt["next_action"]
                .as_str()
                .unwrap_or_default()
                .contains("Refresh registry evidence")
        );
    }

    #[test]
    fn dogfood_value_subjects_caps_error_group_probe_fanout() {
        let targets = json!({"ok": true, "response": {"targets": []}});
        let monitors = json!({"ok": true, "response": {"monitors": []}});
        let query = json!({"ok": true, "response": {"groups": [
            {"group_hash": "grp-low", "count": 1},
            {"group_hash": "grp-high", "count": 30},
            {"group_hash": "grp-mid", "count": 12},
            {"group_hash": "grp-total", "total_count": 20},
            {"group_hash": "grp-over-limit", "count": 2}
        ]}});
        let incidents = json!({"ok": true, "response": {"incidents": []}});

        let subjects = dogfood_value_subjects("linejam", &targets, &monitors, &query, &incidents);
        let error_group_ids = subjects
            .iter()
            .filter(|subject| subject.subject_type == "error_group")
            .map(|subject| subject.subject_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(error_group_ids, vec!["grp-high", "grp-total", "grp-mid"]);
    }

    #[test]
    fn dogfood_value_summary_counts_value_coverage_shapes() {
        let inventory = json!({
            "summary": {"covered": 2, "partial": 1, "blocked": 1, "ignored": 1},
            "surfaces": [
                {"service": "linejam", "coverage": "covered", "evidence_stale": false, "receipt_seen": true},
                {"service": "chrondle", "coverage": "covered", "evidence_stale": true, "receipt_seen": false},
                {"service": "reader", "coverage": "partial", "receipt_seen": false},
                {"service": "blocked", "coverage": "blocked", "receipt_seen": false},
                {"service": "archive", "coverage": "ignored", "receipt_seen": false}
            ]
        });

        let summary = dogfood_value_summary(&inventory);

        assert_eq!(summary["covered"], 2);
        assert_eq!(summary["stale"], 1);
        assert_eq!(summary["blocked"], 1);
        assert_eq!(summary["partial"], 1);
        assert_eq!(summary["value_unproven"], 3);
        assert_eq!(summary["ignored"], 1);
    }

    #[test]
    fn mcp_manifest_exposes_integration_tools() {
        let manifest = tool_manifest();
        let names = manifest
            .iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();

        assert!(names.contains("canary_integrate_discover"));
        assert!(names.contains("canary_integrate_status"));
        assert!(names.contains("canary_integrate_plan"));
        assert!(names.contains("canary_integrate_patch"));
        assert!(names.contains("canary_integrate_enroll"));
        assert!(names.contains("canary_event_capture"));
        assert!(names.contains("canary_claims_list"));
        assert!(names.contains("canary_claim_get"));
        assert!(names.contains("canary_claim_create"));
        assert!(names.contains("canary_claim_transition"));
        assert!(names.contains("canary_claim_release"));
        assert!(names.contains("canary_dogfood_value"));
        let tool_by_name = manifest
            .iter()
            .map(|tool| (tool.name, tool))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            tool_by_name["canary_dogfood_value"].input_schema["required"],
            json!(["service"])
        );
        assert_eq!(
            tool_by_name["canary_claim_get"].input_schema["required"],
            json!(["claim_id"])
        );
        assert_eq!(
            tool_by_name["canary_event_capture"].input_schema["required"],
            json!(["service", "name", "summary"])
        );
        assert_eq!(
            tool_by_name["canary_claim_create"].input_schema["required"],
            json!([
                "subject_type",
                "subject_id",
                "owner",
                "purpose",
                "ttl_ms",
                "idempotency_key"
            ])
        );
        assert_eq!(
            tool_by_name["canary_claims_list"].input_schema["properties"]["limit"]["maximum"],
            json!(50)
        );
        assert!(
            tool_by_name["canary_claims_list"].input_schema["properties"]
                .get("cursor")
                .is_some()
        );
        assert_eq!(
            tool_by_name["canary_claim_transition"].input_schema["required"],
            json!(["claim_id", "owner", "state"])
        );
        assert_eq!(
            tool_by_name["canary_claim_release"].input_schema["required"],
            json!(["claim_id", "owner"])
        );
        for tool in manifest {
            assert_eq!(
                tool.input_schema["type"], "object",
                "{} should declare an object input schema",
                tool.name
            );
            assert!(
                tool.input_schema.get("properties").is_some(),
                "{} should declare properties for agents",
                tool.name
            );
        }
    }

    fn temp_project(name: &str) -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = env::temp_dir().join(format!("canary-cli-{name}-{nonce}"));
        fs::create_dir_all(&root)?;
        Ok(root)
    }

    fn has_action(actions: &[Value], kind: &str, status: &str) -> bool {
        actions
            .iter()
            .any(|item| item["kind"] == kind && item["status"] == status)
    }

    fn action<'a>(
        actions: &'a [Value],
        kind: &str,
    ) -> std::result::Result<&'a Value, Box<dyn std::error::Error>> {
        actions
            .iter()
            .find(|item| item["kind"] == kind)
            .ok_or_else(|| format!("missing action {kind}").into())
    }
}
