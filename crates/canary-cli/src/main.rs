//! Command-line entrypoint for agent-native Canary inspection.

use std::{env, path::PathBuf, process::ExitCode};

use canary_cli::{
    ApiClient, CliError, Config, RenderMode, Result, Window, doctor_report,
    dogfood_strict_failure_count, encode, find_repo_root, json_envelope, print_json, print_lines,
    resolve_endpoint_without_config, run_dogfood_inventory, summarize_doctor, summarize_dogfood,
    summarize_incidents, summarize_monitors, summarize_query, summarize_report, summarize_services,
    summarize_targets, summarize_timeline, tool_manifest,
};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

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
    /// Inspect recent errors for one service.
    Errors(ServiceWindowArgs),
    /// List active incidents.
    Incidents(IncidentsArgs),
    /// Inspect timeline events.
    Timeline(TimelineArgs),
    /// List configured HTTP targets.
    Targets,
    /// List configured non-HTTP monitors.
    Monitors,
    /// Inspect dogfood coverage.
    Dogfood(DogfoodArgs),
    /// Run an agent-oriented health and coverage diagnostic.
    Doctor,
    /// Emit the CLI-backed MCP tool manifest.
    McpManifest,
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
struct IncidentsArgs {
    #[arg(long)]
    open: bool,
}

#[derive(Debug, Args)]
struct TimelineArgs {
    #[arg(long)]
    service: Option<String>,
    #[arg(long, default_value = "24h")]
    window: String,
    #[arg(long, default_value_t = 20)]
    limit: u16,
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
}

#[derive(Debug, Args)]
struct DogfoodAuditArgs {
    #[arg(long)]
    strict: bool,
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
        },
        Commands::McpManifest => print_json(&json!({
            "schema_version": 1,
            "tools": tool_manifest()
        })),
        command => run_http_command(command, endpoint, api_key, config, mode),
    }
}

fn run_http_command(
    command: Commands,
    endpoint: Option<String>,
    api_key: Option<String>,
    config_path: Option<PathBuf>,
    mode: RenderMode,
) -> Result<()> {
    let config = Config::resolve(endpoint, api_key, config_path)?;
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
        Commands::Errors(args) => {
            let window = Window::parse(&args.window)?;
            let path = format!(
                "/api/v1/query?service={}&window={}",
                encode(&args.service),
                window.as_str()
            );
            let response = client.get_auth_json(&path)?;
            render("errors", client.endpoint(), response, mode, summarize_query)
        }
        Commands::Incidents(_args) => {
            let response = client.get_auth_json("/api/v1/incidents")?;
            render(
                "incidents",
                client.endpoint(),
                response,
                mode,
                summarize_incidents,
            )
        }
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
        Commands::Dogfood(_) | Commands::McpManifest => unreachable!("handled before HTTP setup"),
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
