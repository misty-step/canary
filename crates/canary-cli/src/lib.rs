//! Agent-native inspection helpers for the `canary` CLI.

use std::{
    collections::{BTreeSet, VecDeque},
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
const DEFAULT_INTEGRATION_ENDPOINT_ENV: &str = "CANARY_ENDPOINT";
const DEFAULT_INTEGRATION_SERVER_KEY_ENV: &str = "CANARY_API_KEY";
const DEFAULT_INTEGRATION_PUBLIC_ENDPOINT_ENV: &str = "NEXT_PUBLIC_CANARY_ENDPOINT";
const DEFAULT_INTEGRATION_PUBLIC_KEY_ENV: &str = "NEXT_PUBLIC_CANARY_API_KEY";

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
    let env_names = discover_env_names(&project_root)?;
    let code_paths = discover_code_paths(&project_root)?;
    let health_routes = discover_health_routes(&project_root);
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
            "env_names": env_names,
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
    let health_url = production_url.map(|url| format!("{}/api/health", url.trim_end_matches('/')));
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
    actions.push(json!({
        "kind": "webhook_subscription",
        "status": "manual",
        "description": "Optional responder webhook; keep repo mutation outside Canary.",
        "executor": "admin-api",
        "events": ["error.new_class", "incident.opened", "health_check.failed"]
    }));

    Ok(json!({
        "schema_version": 1,
        "target": target.display().to_string(),
        "service": service,
        "framework": framework,
        "can_patch": path_exists && framework == "nextjs",
        "discovery": discovery,
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
    if request.redact {
        Ok(redact_secret_value(response))
    } else {
        Ok(response)
    }
}

/// Summarize an integration discovery/plan/patch/enroll response.
pub fn summarize_integration(value: &Value) -> Vec<String> {
    let mut lines = vec![
        format!("service: {}", string_field(value, "service")),
        format!("framework: {}", string_field(value, "framework")),
    ];
    if let Some(actions) = value.get("actions").and_then(Value::as_array) {
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
            if path.is_dir() {
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
            if body.contains("@canary-obs/sdk") || body.contains("initCanary") {
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
    let mut routes = Vec::new();
    for (path, route) in [
        ("app/api/health/route.ts", "/api/health"),
        ("app/api/health/route.tsx", "/api/health"),
        ("pages/api/health.ts", "/api/health"),
        ("pages/api/health.js", "/api/health"),
        ("app/health/route.ts", "/health"),
        ("app/health/route.tsx", "/health"),
    ] {
        if root.join(path).is_file() {
            routes.push(json!({"path": path, "route": route}));
        }
    }
    routes
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
        "worker_readiness": worker_readiness_report(&readyz)
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
    lines.push(format!(
        "worker_readiness: {}",
        worker_readiness_summary(value.get("worker_readiness"))
    ));
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

fn worker_readiness_report(readyz_probe: &Value) -> Value {
    if !probe_ok(readyz_probe) {
        return json!({
            "available": false,
            "reason": string_field(readyz_probe, "error")
        });
    }

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

    let failing_workers = workers
        .iter()
        .filter(|worker| {
            worker.get("state").and_then(Value::as_str) != Some("started")
                || worker
                    .get("failure_count")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    > 0
        })
        .count();

    json!({
        "available": true,
        "status": response
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        "worker_count": workers.len(),
        "failing_workers": failing_workers,
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
                        .filter(|worker| {
                            worker.get("state").and_then(Value::as_str) != Some("started")
                                || worker
                                    .get("failure_count")
                                    .and_then(Value::as_i64)
                                    .unwrap_or(0)
                                    > 0
                        })
                        .count() as i64
                })
        });
    format!(
        "{} {} workers, {} failing",
        string_field(value, "status"),
        worker_count,
        failing_workers
    )
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
        ToolSpec {
            name: "canary_integrate_discover",
            description: "Discover local project integration state without reading secret values.",
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
            input_schema: json!({"type":"object","required":["service","url"],"properties":{"service":{"type":"string"},"url":{"type":"string"},"environment":{"type":"string"},"interval_ms":{"type":"integer"},"show_secret":{"type":"boolean","default":false}}}),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn mcp_manifest_exposes_integration_tools() {
        let manifest = tool_manifest();
        let names = manifest
            .iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();

        assert!(names.contains("canary_integrate_discover"));
        assert!(names.contains("canary_integrate_plan"));
        assert!(names.contains("canary_integrate_patch"));
        assert!(names.contains("canary_integrate_enroll"));
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
