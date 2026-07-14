//! Command-line entrypoint for agent-native Canary inspection.

use std::{
    env,
    io::{self, BufRead, Write},
    path::PathBuf,
    process::ExitCode,
};

use canary_cli::{
    ApiClient, CliError, Config, IntegrationEnrollRequest, IntegrationInput, McpToolContext,
    RenderMode, Result, Window, doctor_report, dogfood_strict_failure_count, dogfood_value_report,
    encode, find_repo_root, integration_discover, integration_enroll, integration_patch,
    integration_plan, integration_status, json_envelope, mcp_tool_manifest,
    normalize_event_payload, print_json, print_lines, resolve_endpoint_without_config,
    run_dogfood_inventory, summarize_annotations, summarize_claims, summarize_doctor,
    summarize_dogfood, summarize_dogfood_value, summarize_error_detail, summarize_event,
    summarize_incident_detail, summarize_incident_escalation, summarize_incidents,
    summarize_integration, summarize_monitors, summarize_query, summarize_report,
    summarize_services, summarize_targets, summarize_timeline, summarize_webhook_delivery,
    tool_manifest,
};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

#[derive(Debug, Parser)]
#[command(name = "canary")]
#[command(about = "Agent-native inspection CLI for Canary")]
struct Cli {
    #[arg(long, global = true)]
    endpoint: Option<String>,
    #[arg(long, global = true)]
    api_key: Option<String>,
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Inspect global Canary status.
    Summary(WindowArgs),
    /// List target and monitor service states.
    Services(ServicesArgs),
    /// List or read recent errors.
    Errors(ErrorsArgs),
    /// List, read, escalate, or deescalate incidents.
    Incidents(IncidentsArgs),
    /// Inspect timeline events.
    Timeline(TimelineArgs),
    /// Read webhook delivery diagnostics.
    WebhookDeliveries(WebhookDeliveriesArgs),
    /// List configured HTTP targets.
    Targets,
    /// List configured non-HTTP monitors.
    Monitors,
    /// Inspect dogfood coverage.
    Dogfood(DogfoodArgs),
    /// Capture bounded telemetry or operational events.
    Events(EventsArgs),
    /// Coordinate agentic remediation claims.
    Claims(ClaimsArgs),
    /// Read or write coordination annotations.
    Annotations(AnnotationsArgs),
    /// Discover, plan, patch, and enroll application integrations.
    Integrate(IntegrateArgs),
    /// Run an agent-oriented health and coverage diagnostic.
    Doctor,
    /// Emit the CLI-backed MCP tool manifest.
    McpManifest,
    /// Run the CLI-backed MCP stdio server.
    McpServer,
}

#[derive(Debug, Args)]
struct WindowArgs {
    #[arg(long, default_value = "24h")]
    window: String,
}

#[derive(Debug, Args)]
struct ServicesArgs {
    #[arg(long)]
    state: Option<String>,
    #[arg(long, default_value = "24h")]
    window: String,
}

#[derive(Debug, Args)]
struct ServiceWindowArgs {
    service: String,
    #[arg(long, default_value = "24h")]
    window: String,
}

#[derive(Debug, Args)]
struct ErrorsArgs {
    #[command(subcommand)]
    command: ErrorsCommand,
}

#[derive(Debug, Subcommand)]
enum ErrorsCommand {
    /// List recent error groups for one service.
    List(ServiceWindowArgs),
    /// Read one raw error by id, including stack trace and decoded context.
    Get(ErrorGetArgs),
}

#[derive(Debug, Args)]
struct ErrorGetArgs {
    error_id: String,
}

#[derive(Debug, Args)]
struct WebhookDeliveriesArgs {
    #[command(subcommand)]
    command: WebhookDeliveriesCommand,
}

#[derive(Debug, Subcommand)]
enum WebhookDeliveriesCommand {
    /// Read one webhook delivery by stable delivery id.
    Get(WebhookDeliveryGetArgs),
}

#[derive(Debug, Args)]
struct WebhookDeliveryGetArgs {
    delivery_id: String,
}

#[derive(Debug, Args)]
struct IncidentsArgs {
    #[command(subcommand)]
    command: IncidentsCommand,
}

#[derive(Debug, Subcommand)]
enum IncidentsCommand {
    /// List active incidents.
    List(IncidentsListArgs),
    /// Read one incident detail by id.
    Get(IncidentGetArgs),
    /// Escalate one incident for human paging.
    Escalate(IncidentEscalateArgs),
    /// Clear a false-positive escalation.
    Deescalate(IncidentDeescalateArgs),
}

#[derive(Debug, Args)]
struct IncidentsListArgs {
    #[arg(long)]
    open: bool,
}

#[derive(Debug, Args)]
struct IncidentGetArgs {
    incident_id: String,
}

#[derive(Debug, Args)]
struct IncidentEscalateArgs {
    incident_id: String,
    #[arg(long)]
    reason: String,
    #[arg(long)]
    owner: String,
    #[arg(long)]
    purpose: String,
    #[arg(long)]
    idempotency_key: String,
}

#[derive(Debug, Args)]
struct IncidentDeescalateArgs {
    incident_id: String,
    #[arg(long)]
    owner: String,
    #[arg(long)]
    reason: Option<String>,
}

#[derive(Debug, Args)]
struct TimelineArgs {
    #[arg(long)]
    service: Option<String>,
    #[arg(long, default_value = "24h")]
    window: String,
    #[arg(long, default_value_t = 20)]
    limit: u16,
    #[arg(long)]
    cursor: Option<String>,
    #[arg(long)]
    after: Option<String>,
}

#[derive(Debug, Args)]
struct DogfoodArgs {
    #[command(subcommand)]
    command: DogfoodCommand,
}

#[derive(Debug, Subcommand)]
enum DogfoodCommand {
    /// Run deployed service coverage inventory.
    Audit(DogfoodAuditArgs),
    /// Build one per-service value receipt from dogfood and live Canary evidence.
    Value(DogfoodValueArgs),
}

#[derive(Debug, Args)]
struct DogfoodAuditArgs {
    #[arg(long)]
    strict: bool,
}

#[derive(Debug, Args)]
struct DogfoodValueArgs {
    #[arg(long)]
    service: String,
    #[arg(long, default_value = "24h")]
    window: String,
}

#[derive(Debug, Args)]
struct EventsArgs {
    #[command(subcommand)]
    command: EventsCommand,
}

#[derive(Debug, Subcommand)]
enum EventsCommand {
    /// Capture one telemetry or operational event.
    Capture(EventCaptureArgs),
}

#[derive(Debug, Args)]
struct EventCaptureArgs {
    #[arg(long)]
    service: String,
    #[arg(long)]
    name: String,
    #[arg(long)]
    summary: String,
    #[arg(long, default_value = "info")]
    severity: String,
    #[arg(long = "attribute")]
    attributes: Vec<String>,
    #[arg(long)]
    retention_class: Option<String>,
    #[arg(long, default_value = "redacted")]
    privacy_policy: String,
    #[arg(long, default_value = "unsampled")]
    sampling_policy: String,
    #[arg(long)]
    operational_subject_type: Option<String>,
    #[arg(long)]
    operational_subject_id: Option<String>,
    #[arg(long)]
    operational_state: Option<String>,
    #[arg(long)]
    operational_owner: Option<String>,
    #[arg(long)]
    operational_evidence_url: Option<String>,
    #[arg(long)]
    operational_observed_at: Option<String>,
}

#[derive(Debug, Args)]
struct ClaimsArgs {
    #[command(subcommand)]
    command: ClaimsCommand,
}

#[derive(Debug, Subcommand)]
enum ClaimsCommand {
    /// List claims for one subject.
    List(ClaimSubjectArgs),
    /// Read one claim by id.
    Get(ClaimIdArgs),
    /// Claim one subject.
    Claim(ClaimCreateArgs),
    /// Transition one claim to a bounded state.
    Transition(ClaimTransitionArgs),
    /// Release one claim.
    Release(ClaimReleaseArgs),
}

#[derive(Debug, Args)]
struct ClaimSubjectArgs {
    #[arg(long)]
    subject_type: String,
    #[arg(long)]
    subject_id: String,
    #[arg(long)]
    limit: Option<u16>,
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct ClaimCreateArgs {
    #[arg(long)]
    subject_type: String,
    #[arg(long)]
    subject_id: String,
    #[arg(long)]
    owner: String,
    #[arg(long)]
    purpose: String,
    #[arg(long)]
    idempotency_key: String,
    #[arg(long, default_value_t = 900_000)]
    ttl_ms: i64,
    #[arg(long = "evidence-link")]
    evidence_links: Vec<String>,
}

#[derive(Debug, Args)]
struct ClaimTransitionArgs {
    claim_id: String,
    #[arg(long)]
    owner: String,
    #[arg(long)]
    state: String,
    #[arg(long = "evidence-link")]
    evidence_links: Vec<String>,
}

#[derive(Debug, Args)]
struct ClaimIdArgs {
    claim_id: String,
}

#[derive(Debug, Args)]
struct ClaimReleaseArgs {
    claim_id: String,
    #[arg(long)]
    owner: String,
}

#[derive(Debug, Args)]
struct AnnotationsArgs {
    #[command(subcommand)]
    command: AnnotationsCommand,
}

#[derive(Debug, Subcommand)]
enum AnnotationsCommand {
    /// List annotations for one subject.
    List(AnnotationSubjectArgs),
    /// Create an annotation for one subject.
    Create(AnnotationCreateArgs),
}

#[derive(Debug, Args)]
struct AnnotationSubjectArgs {
    #[arg(long)]
    subject_type: String,
    #[arg(long)]
    subject_id: String,
    #[arg(long)]
    limit: Option<u16>,
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct AnnotationCreateArgs {
    #[arg(long)]
    subject_type: String,
    #[arg(long)]
    subject_id: String,
    #[arg(long)]
    agent: String,
    #[arg(long)]
    action: String,
    #[arg(long = "metadata")]
    metadata: Vec<String>,
}

#[derive(Debug, Args)]
struct IntegrateArgs {
    #[command(subcommand)]
    command: IntegrateCommand,
}

#[derive(Debug, Subcommand)]
enum IntegrateCommand {
    /// Merge local, receipt, platform, live Canary, and dogfood evidence into one verdict.
    Status(IntegrateProjectArgs),
    /// Discover local integration state without reading secret values.
    Discover(IntegrateProjectArgs),
    /// Emit a reviewable patch/enrollment plan.
    Plan(IntegrateProjectArgs),
    /// Apply safe Next.js integration patches.
    Patch(IntegrateProjectArgs),
    /// Enroll a deployed service in Canary.
    Enroll(IntegrateEnrollArgs),
}

#[derive(Debug, Args)]
struct IntegrateProjectArgs {
    path_or_project: PathBuf,
    #[arg(long)]
    service: Option<String>,
    #[arg(long)]
    production_url: Option<String>,
    #[arg(long)]
    platform_project: Option<String>,
}

#[derive(Debug, Args)]
struct IntegrateEnrollArgs {
    #[arg(long)]
    service: String,
    #[arg(long)]
    url: String,
    #[arg(long, default_value = "production")]
    environment: String,
    #[arg(long)]
    interval_ms: Option<i64>,
    #[arg(long)]
    project_root: Option<PathBuf>,
    #[arg(long)]
    show_secret: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("canary: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<()> {
    let Cli {
        endpoint,
        api_key,
        config,
        json,
        command,
    } = Cli::parse();
    let mode = if json {
        RenderMode::Json
    } else {
        RenderMode::Text
    };

    match command {
        Commands::Dogfood(args) => match args.command {
            DogfoodCommand::Audit(audit_args) => {
                let cwd = env::current_dir().map_err(|source| CliError::Io {
                    path: PathBuf::from("."),
                    source,
                })?;
                let repo_root = find_repo_root(&cwd)?;
                let endpoint = resolve_endpoint_without_config(endpoint.as_deref());
                let response = run_dogfood_inventory(&repo_root, false)?;
                let strict_failures = dogfood_strict_failure_count(&response);
                render(
                    "dogfood audit",
                    &endpoint,
                    response,
                    mode,
                    summarize_dogfood,
                )?;
                if audit_args.strict && strict_failures > 0 {
                    return Err(CliError::Message(format!(
                        "dogfood strict failures: {strict_failures}"
                    )));
                }
                Ok(())
            }
            DogfoodCommand::Value(value_args) => {
                let config = Config::resolve(endpoint, api_key, config)?;
                let client = ApiClient::new(config)?;
                let cwd = env::current_dir().map_err(|source| CliError::Io {
                    path: PathBuf::from("."),
                    source,
                })?;
                let repo_root = find_repo_root(&cwd)?;
                let window = Window::parse(&value_args.window)?;
                let response =
                    dogfood_value_report(&client, &repo_root, &value_args.service, window)?;
                render(
                    "dogfood value",
                    client.endpoint(),
                    response,
                    mode,
                    summarize_dogfood_value,
                )
            }
        },
        Commands::Integrate(args) => run_integrate_command(args, endpoint, api_key, config, mode),
        Commands::McpManifest => print_json(&json!({
            "schema_version": 1,
            "tools": tool_manifest()
        })),
        Commands::McpServer => run_mcp_server(endpoint, api_key, config),
        command => run_http_command(command, endpoint, api_key, config, mode),
    }
}

fn run_integrate_command(
    args: IntegrateArgs,
    endpoint: Option<String>,
    api_key: Option<String>,
    config_path: Option<PathBuf>,
    mode: RenderMode,
) -> Result<()> {
    match args.command {
        IntegrateCommand::Discover(args) => {
            let endpoint = resolve_endpoint_without_config(endpoint.as_deref());
            let input = integration_input(args, endpoint.clone());
            let response = integration_discover(&input)?;
            render(
                "integrate discover",
                &endpoint,
                response,
                mode,
                summarize_integration,
            )
        }
        IntegrateCommand::Status(args) => {
            let config = Config::resolve(endpoint, api_key, config_path)?;
            let client = ApiClient::new(config)?;
            let cwd = env::current_dir().map_err(|source| CliError::Io {
                path: PathBuf::from("."),
                source,
            })?;
            let repo_root = find_repo_root(&cwd).unwrap_or(cwd);
            let input = integration_input(args, client.endpoint().to_owned());
            let response = integration_status(&client, &input, &repo_root)?;
            render(
                "integrate status",
                client.endpoint(),
                response,
                mode,
                summarize_integration,
            )
        }
        IntegrateCommand::Plan(args) => {
            let endpoint = resolve_endpoint_without_config(endpoint.as_deref());
            let input = integration_input(args, endpoint.clone());
            let response = integration_plan(&input)?;
            render(
                "integrate plan",
                &endpoint,
                response,
                mode,
                summarize_integration,
            )
        }
        IntegrateCommand::Patch(args) => {
            let endpoint = resolve_endpoint_without_config(endpoint.as_deref());
            let input = integration_input(args, endpoint.clone());
            let response = integration_patch(&input)?;
            render(
                "integrate patch",
                &endpoint,
                response,
                mode,
                summarize_integration,
            )
        }
        IntegrateCommand::Enroll(args) => {
            let config = Config::resolve(endpoint, api_key, config_path)?;
            let client = ApiClient::new(config)?;
            let request = IntegrationEnrollRequest {
                service: args.service,
                url: args.url,
                environment: args.environment,
                interval_ms: args.interval_ms,
                redact: !args.show_secret,
                receipt_root: args.project_root,
            };
            let response = integration_enroll(&client, &request)?;
            render(
                "integrate enroll",
                client.endpoint(),
                response,
                mode,
                summarize_integration,
            )
        }
    }
}

fn integration_input(args: IntegrateProjectArgs, endpoint: String) -> IntegrationInput {
    IntegrationInput {
        target: args.path_or_project,
        service: args.service,
        production_url: args.production_url,
        platform_project: args.platform_project,
        endpoint,
    }
}

fn run_http_command(
    command: Commands,
    endpoint: Option<String>,
    api_key: Option<String>,
    config_path: Option<PathBuf>,
    mode: RenderMode,
) -> Result<()> {
    match command {
        Commands::Claims(args) => run_claims_command(args, endpoint, api_key, config_path, mode),
        Commands::Annotations(args) => {
            run_annotations_command(args, endpoint, api_key, config_path, mode)
        }
        Commands::Incidents(args) => {
            run_incidents_command(args, endpoint, api_key, config_path, mode)
        }
        command => {
            let config = match &command {
                Commands::Events(_) => Config::resolve_for_ingest(endpoint, api_key, config_path)?,
                _ => Config::resolve(endpoint, api_key, config_path)?,
            };
            let client = ApiClient::new(config)?;

            match command {
                Commands::Summary(args) => {
                    let window = Window::parse(&args.window)?;
                    let path = format!("/api/v1/report?window={}", window.as_str());
                    let response = client.get_auth_json(&path)?;
                    render(
                        "summary",
                        client.endpoint(),
                        response,
                        mode,
                        summarize_report,
                    )
                }
                Commands::Services(args) => {
                    let window = Window::parse(&args.window)?;
                    let path = format!("/api/v1/status?window={}", window.as_str());
                    let response = client.get_auth_json(&path)?;
                    let state = args.state;
                    render_with_lines("services", client.endpoint(), response, mode, |value| {
                        summarize_services(value, state.as_deref())
                    })
                }
                Commands::Errors(args) => match args.command {
                    ErrorsCommand::List(list_args) => {
                        let window = Window::parse(&list_args.window)?;
                        let path = format!(
                            "/api/v1/query?service={}&window={}",
                            encode(&list_args.service),
                            window.as_str()
                        );
                        let response = client.get_auth_json(&path)?;
                        render("errors", client.endpoint(), response, mode, summarize_query)
                    }
                    ErrorsCommand::Get(get_args) => {
                        let response = client.get_auth_json(&format!(
                            "/api/v1/errors/{}",
                            encode(&get_args.error_id)
                        ))?;
                        render(
                            "errors get",
                            client.endpoint(),
                            response,
                            mode,
                            summarize_error_detail,
                        )
                    }
                },
                Commands::WebhookDeliveries(args) => match args.command {
                    WebhookDeliveriesCommand::Get(get_args) => {
                        let response = client.get_auth_json(&format!(
                            "/api/v1/webhook-deliveries/{}",
                            encode(&get_args.delivery_id)
                        ))?;
                        render(
                            "webhook-deliveries get",
                            client.endpoint(),
                            response,
                            mode,
                            summarize_webhook_delivery,
                        )
                    }
                },
                Commands::Timeline(args) => {
                    let window = Window::parse(&args.window)?;
                    let mut path = format!(
                        "/api/v1/timeline?window={}&limit={}",
                        window.as_str(),
                        args.limit
                    );
                    if let Some(service) = args.service {
                        path.push_str("&service=");
                        path.push_str(&encode(&service));
                    }
                    if let Some(cursor) = args.cursor {
                        path.push_str("&cursor=");
                        path.push_str(&encode(&cursor));
                    }
                    if let Some(after) = args.after {
                        path.push_str("&after=");
                        path.push_str(&encode(&after));
                    }
                    let response = client.get_auth_json(&path)?;
                    render(
                        "timeline",
                        client.endpoint(),
                        response,
                        mode,
                        summarize_timeline,
                    )
                }
                Commands::Targets => {
                    let response = client.get_auth_json("/api/v1/targets")?;
                    render(
                        "targets",
                        client.endpoint(),
                        response,
                        mode,
                        summarize_targets,
                    )
                }
                Commands::Monitors => {
                    let response = client.get_auth_json("/api/v1/monitors")?;
                    render(
                        "monitors",
                        client.endpoint(),
                        response,
                        mode,
                        summarize_monitors,
                    )
                }
                Commands::Events(args) => run_events_command(args, &client, mode),
                Commands::Doctor => {
                    let cwd = env::current_dir().map_err(|source| CliError::Io {
                        path: PathBuf::from("."),
                        source,
                    })?;
                    let repo_root = find_repo_root(&cwd)?;
                    let response = doctor_report(&client, &repo_root);
                    render(
                        "doctor",
                        client.endpoint(),
                        response,
                        mode,
                        summarize_doctor,
                    )
                }
                Commands::Dogfood(_)
                | Commands::Integrate(_)
                | Commands::McpManifest
                | Commands::McpServer
                | Commands::Claims(_)
                | Commands::Annotations(_)
                | Commands::Incidents(_) => {
                    unreachable!("handled before HTTP setup")
                }
            }
        }
    }
}

fn run_mcp_server(
    endpoint: Option<String>,
    api_key: Option<String>,
    config_path: Option<PathBuf>,
) -> Result<()> {
    let cwd = env::current_dir().map_err(|source| CliError::Io {
        path: PathBuf::from("."),
        source,
    })?;
    let repo_root = find_repo_root(&cwd).unwrap_or(cwd);
    let context = McpToolContext::new(endpoint, api_key, config_path, repo_root);
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = line.map_err(|source| CliError::Io {
            path: PathBuf::from("<stdin>"),
            source,
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle_mcp_message(&context, &message),
            Err(error) => Some(jsonrpc_error(Value::Null, -32700, &error.to_string())),
        };
        if let Some(response) = response {
            writeln!(stdout, "{response}").map_err(|source| CliError::Io {
                path: PathBuf::from("<stdout>"),
                source,
            })?;
            stdout.flush().map_err(|source| CliError::Io {
                path: PathBuf::from("<stdout>"),
                source,
            })?;
        }
    }

    Ok(())
}

fn handle_mcp_message(context: &McpToolContext, message: &Value) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let id = message.get("id").cloned()?;

    match method {
        "initialize" => Some(jsonrpc_result(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    }
                },
                "serverInfo": {
                    "name": "canary",
                    "title": "Canary",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": "Use Canary tools for agent-first production health inspection, remediation claims, and annotation writeback. Webhooks are wake-up hints; timeline/report replay is the correctness path."
            }),
        )),
        "ping" => Some(jsonrpc_result(id, json!({}))),
        "tools/list" => Some(jsonrpc_result(
            id,
            json!({
                "tools": mcp_tool_manifest()
            }),
        )),
        "tools/call" => Some(handle_mcp_tool_call(context, id, message)),
        _ => Some(jsonrpc_error(
            id,
            -32601,
            &format!("method not found: {method}"),
        )),
    }
}

fn handle_mcp_tool_call(context: &McpToolContext, id: Value, message: &Value) -> Value {
    let Some(params) = message.get("params").and_then(Value::as_object) else {
        return jsonrpc_error(id, -32602, "tools/call params must be an object");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return jsonrpc_error(id, -32602, "tools/call params.name must be a string");
    };
    let empty_arguments = json!({});
    let arguments = params.get("arguments").unwrap_or(&empty_arguments);

    let result = match context.invoke(name, arguments) {
        Ok(value) => mcp_tool_result(value),
        Err(error) => mcp_tool_error(error.to_string()),
    };
    jsonrpc_result(id, result)
}

fn mcp_tool_result(value: Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": value_to_pretty_text(&value)
        }],
        "structuredContent": value
    })
}

fn mcp_tool_error(message: String) -> Value {
    let structured_content = json!({
        "schema_version": 1,
        "error": {
            "message": message.clone()
        }
    });
    json!({
        "content": [{
            "type": "text",
            "text": message
        }],
        "structuredContent": structured_content,
        "isError": true
    })
}

fn value_to_pretty_text(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn jsonrpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn jsonrpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn run_events_command(args: EventsArgs, client: &ApiClient, mode: RenderMode) -> Result<()> {
    match args.command {
        EventsCommand::Capture(args) => {
            let operational = operational_event_payload(&args)?;
            let retention_class = args.retention_class.clone().unwrap_or_else(|| {
                if operational.is_some() {
                    "audit".to_owned()
                } else {
                    "standard".to_owned()
                }
            });
            let mut payload = json!({
                "service": args.service,
                "name": args.name,
                "summary": args.summary,
                "severity": args.severity,
                "attributes": parse_attributes(&args.attributes)?,
                "retention_class": retention_class,
                "privacy_policy": args.privacy_policy,
                "sampling_policy": args.sampling_policy,
            });
            if let Some(operational) = operational {
                payload["operational"] = operational;
            }
            let payload = normalize_event_payload(payload)?;
            let response = client.post_auth_json("/api/v1/events", &payload)?;
            render(
                "events capture",
                client.endpoint(),
                response,
                mode,
                summarize_event,
            )
        }
    }
}

fn operational_event_payload(args: &EventCaptureArgs) -> Result<Option<Value>> {
    let values = [
        args.operational_subject_type.as_deref(),
        args.operational_subject_id.as_deref(),
        args.operational_state.as_deref(),
        args.operational_owner.as_deref(),
        args.operational_evidence_url.as_deref(),
        args.operational_observed_at.as_deref(),
    ];
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    if values.iter().any(Option::is_none) {
        return Err(CliError::Message(
            "operational events require --operational-subject-type, --operational-subject-id, --operational-state, --operational-owner, --operational-evidence-url, and --operational-observed-at"
                .to_owned(),
        ));
    }
    Ok(Some(json!({
        "subject": {
            "type": args.operational_subject_type,
            "id": args.operational_subject_id,
        },
        "state": args.operational_state,
        "owner": args.operational_owner,
        "evidence_url": args.operational_evidence_url,
        "observed_at": args.operational_observed_at,
    })))
}

fn parse_attributes(attributes: &[String]) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut object = serde_json::Map::new();
    for attribute in attributes {
        let Some((key, value)) = attribute.split_once('=') else {
            return Err(CliError::Message(format!(
                "attribute must be key=value: {attribute}"
            )));
        };
        if key.is_empty() {
            return Err(CliError::Message(
                "attribute key must not be empty".to_owned(),
            ));
        }
        object.insert(key.to_owned(), serde_json::Value::String(value.to_owned()));
    }
    Ok(object)
}

fn run_claims_command(
    args: ClaimsArgs,
    endpoint: Option<String>,
    api_key: Option<String>,
    config_path: Option<PathBuf>,
    mode: RenderMode,
) -> Result<()> {
    match args.command {
        ClaimsCommand::List(args) => {
            let client = ApiClient::new(Config::resolve(endpoint, api_key, config_path)?)?;
            let mut path = format!(
                "/api/v1/claims?subject_type={}&subject_id={}",
                encode(&args.subject_type),
                encode(&args.subject_id)
            );
            if let Some(limit) = args.limit {
                path.push_str(&format!("&limit={limit}"));
            }
            if let Some(cursor) = args.cursor {
                path.push_str(&format!("&cursor={}", encode(&cursor)));
            }
            let response = client.get_auth_json(&path)?;
            render(
                "claims list",
                client.endpoint(),
                response,
                mode,
                summarize_claims,
            )
        }
        ClaimsCommand::Claim(args) => {
            let client = ApiClient::new(Config::resolve_for_responder_write(
                endpoint,
                api_key,
                config_path,
            )?)?;
            let response = client.post_auth_json(
                "/api/v1/claims",
                &json!({
                    "subject_type": args.subject_type,
                    "subject_id": args.subject_id,
                    "owner": args.owner,
                    "purpose": args.purpose,
                    "idempotency_key": args.idempotency_key,
                    "ttl_ms": args.ttl_ms,
                    "evidence_links": args.evidence_links,
                }),
            )?;
            render(
                "claims claim",
                client.endpoint(),
                response,
                mode,
                summarize_claims,
            )
        }
        ClaimsCommand::Get(args) => {
            let client = ApiClient::new(Config::resolve(endpoint, api_key, config_path)?)?;
            let response =
                client.get_auth_json(&format!("/api/v1/claims/{}", encode(&args.claim_id)))?;
            render(
                "claims get",
                client.endpoint(),
                response,
                mode,
                summarize_claims,
            )
        }
        ClaimsCommand::Transition(args) => {
            let client = ApiClient::new(Config::resolve_for_responder_write(
                endpoint,
                api_key,
                config_path,
            )?)?;
            let response = client.post_auth_json(
                &format!("/api/v1/claims/{}/transition", encode(&args.claim_id)),
                &json!({
                    "owner": args.owner,
                    "state": args.state,
                    "evidence_links": args.evidence_links,
                }),
            )?;
            render(
                "claims transition",
                client.endpoint(),
                response,
                mode,
                summarize_claims,
            )
        }
        ClaimsCommand::Release(args) => {
            let client = ApiClient::new(Config::resolve_for_responder_write(
                endpoint,
                api_key,
                config_path,
            )?)?;
            let response = client.post_auth_json(
                &format!("/api/v1/claims/{}/release", encode(&args.claim_id)),
                &json!({
                    "owner": args.owner,
                }),
            )?;
            render(
                "claims release",
                client.endpoint(),
                response,
                mode,
                summarize_claims,
            )
        }
    }
}

fn run_incidents_command(
    args: IncidentsArgs,
    endpoint: Option<String>,
    api_key: Option<String>,
    config_path: Option<PathBuf>,
    mode: RenderMode,
) -> Result<()> {
    match args.command {
        IncidentsCommand::List(_list_args) => {
            let client = ApiClient::new(Config::resolve(endpoint, api_key, config_path)?)?;
            let response = client.get_auth_json("/api/v1/incidents")?;
            render(
                "incidents",
                client.endpoint(),
                response,
                mode,
                summarize_incidents,
            )
        }
        IncidentsCommand::Get(args) => {
            let client = ApiClient::new(Config::resolve(endpoint, api_key, config_path)?)?;
            let response = client
                .get_auth_json(&format!("/api/v1/incidents/{}", encode(&args.incident_id)))?;
            render(
                "incidents get",
                client.endpoint(),
                response,
                mode,
                summarize_incident_detail,
            )
        }
        IncidentsCommand::Escalate(args) => {
            let client = ApiClient::new(Config::resolve_for_responder_write(
                endpoint,
                api_key,
                config_path,
            )?)?;
            let response = client.post_auth_json(
                &format!("/api/v1/incidents/{}/escalate", encode(&args.incident_id)),
                &json!({
                    "reason": args.reason,
                    "owner": args.owner,
                    "purpose": args.purpose,
                    "idempotency_key": args.idempotency_key,
                }),
            )?;
            render(
                "incidents escalate",
                client.endpoint(),
                response,
                mode,
                summarize_incident_escalation,
            )
        }
        IncidentsCommand::Deescalate(args) => {
            let client = ApiClient::new(Config::resolve_for_responder_write(
                endpoint,
                api_key,
                config_path,
            )?)?;
            let mut body = json!({ "owner": args.owner });
            if let Some(reason) = args.reason {
                body["reason"] = json!(reason);
            }
            let response = client.post_auth_json(
                &format!("/api/v1/incidents/{}/deescalate", encode(&args.incident_id)),
                &body,
            )?;
            render(
                "incidents deescalate",
                client.endpoint(),
                response,
                mode,
                summarize_incident_escalation,
            )
        }
    }
}

fn run_annotations_command(
    args: AnnotationsArgs,
    endpoint: Option<String>,
    api_key: Option<String>,
    config_path: Option<PathBuf>,
    mode: RenderMode,
) -> Result<()> {
    match args.command {
        AnnotationsCommand::List(args) => {
            let client = ApiClient::new(Config::resolve(endpoint, api_key, config_path)?)?;
            let mut path = format!(
                "/api/v1/annotations?subject_type={}&subject_id={}",
                encode(&args.subject_type),
                encode(&args.subject_id)
            );
            if let Some(limit) = args.limit {
                path.push_str(&format!("&limit={limit}"));
            }
            if let Some(cursor) = args.cursor {
                path.push_str("&cursor=");
                path.push_str(&encode(&cursor));
            }
            let response = client.get_auth_json(&path)?;
            render(
                "annotations list",
                client.endpoint(),
                response,
                mode,
                summarize_annotations,
            )
        }
        AnnotationsCommand::Create(args) => {
            let client = ApiClient::new(Config::resolve_for_responder_write(
                endpoint,
                api_key,
                config_path,
            )?)?;
            let metadata = parse_attributes(&args.metadata)?;
            let response = client.post_auth_json(
                "/api/v1/annotations",
                &json!({
                    "subject_type": args.subject_type,
                    "subject_id": args.subject_id,
                    "agent": args.agent,
                    "action": args.action,
                    "metadata": metadata,
                }),
            )?;
            render(
                "annotations create",
                client.endpoint(),
                response,
                mode,
                summarize_annotations,
            )
        }
    }
}

fn render(
    command: &str,
    endpoint: &str,
    response: serde_json::Value,
    mode: RenderMode,
    summarize: fn(&serde_json::Value) -> Vec<String>,
) -> Result<()> {
    render_with_lines(command, endpoint, response, mode, summarize)
}

fn render_with_lines(
    command: &str,
    endpoint: &str,
    response: serde_json::Value,
    mode: RenderMode,
    summarize: impl FnOnce(&serde_json::Value) -> Vec<String>,
) -> Result<()> {
    if mode == RenderMode::Json {
        return print_json(&json_envelope(command, endpoint, response));
    }
    print_lines(&summarize(&response));
    Ok(())
}
