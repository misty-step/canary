//! Contract-parity guard (canary-932 child 4): every agent-relevant `GET`
//! operation in the checked-in OpenAPI document must have both a CLI command
//! and an MCP tool, or an explicitly justified allowlist entry. Adding a
//! route to `priv/openapi/openapi.json` without updating `parity_table()`
//! below fails this test with the exact path that needs an entry.
//!
//! It also carries the emitted-command-grammar regression the PR #261 review
//! required: doctor/integrate JSON payloads embed literal `canary ...`
//! command strings as machine-executable remediation hints, and those drift
//! silently when a CLI grammar changes underneath them (canary-932
//! `errors <service>` -> `errors list <service>` broke six such sites). The
//! tests below call the real payload-building functions and prove each
//! embedded command both uses the expected grammar AND resolves as a live
//! CLI subcommand, reusing `cli_subcommand_registered` so this can't drift
//! back to a hardcoded string comparison.

mod support;

use std::{collections::BTreeSet, fs, process::Command};

use canary_cli::{
    ApiClient, Config, DEFAULT_ENDPOINT, IntegrationEnrollRequest, IntegrationInput,
    integration_enroll, integration_patch, integration_plan, next_operator_action, tool_manifest,
};
use serde_json::{Value, json};
use support::{FixtureResponse, FixtureServer};

const OPENAPI_JSON: &str = include_str!("../../../priv/openapi/openapi.json");

/// One parity table entry for one checked-in `GET` operation.
enum Coverage {
    /// Reachable from both the CLI and the MCP tool surface.
    Covered {
        /// Subcommand path as typed after `canary`, e.g. `["errors", "get"]`.
        cli_path: &'static [&'static str],
        /// MCP tool name; must be registered in `tool_manifest()`.
        mcp_tool: &'static str,
    },
    /// Deliberately excluded from CLI/MCP coverage, with a one-line reason.
    Allowlisted { reason: &'static str },
}

/// Declarative path -> coverage table. Every `GET` path in the checked-in
/// OpenAPI document must appear here exactly once.
fn parity_table() -> Vec<(&'static str, Coverage)> {
    use Coverage::{Allowlisted, Covered};
    vec![
        (
            "/healthz",
            Allowlisted {
                reason: "infra liveness probe surfaced via `canary doctor`'s reachability.healthz, not a standalone agent read model",
            },
        ),
        (
            "/readyz",
            Allowlisted {
                reason: "infra readiness probe surfaced via `canary doctor`'s reachability.readyz, not a standalone agent read model",
            },
        ),
        (
            "/api/v1/openapi.json",
            Allowlisted {
                reason: "the contract document itself, not an agent read-model route",
            },
        ),
        (
            "/metrics",
            Allowlisted {
                reason: "admin-only Prometheus scrape target; operator surface, not an agent coordination-loop surface",
            },
        ),
        (
            "/api/v1/query",
            Covered {
                cli_path: &["errors", "list"],
                mcp_tool: "canary_errors",
            },
        ),
        (
            "/api/v1/errors/{id}",
            Covered {
                cli_path: &["errors", "get"],
                mcp_tool: "canary_error_get",
            },
        ),
        (
            "/api/v1/report",
            Covered {
                cli_path: &["summary"],
                mcp_tool: "canary_summary",
            },
        ),
        (
            "/api/v1/timeline",
            Covered {
                cli_path: &["timeline"],
                mcp_tool: "canary_timeline",
            },
        ),
        (
            "/api/v1/webhook-deliveries",
            Allowlisted {
                reason: "list/paginate-by-filter surface not required by the agent replay loop (agents dedupe by x-delivery-id and drill into the singular route below); canary-932 follow-up if a `canary webhook-deliveries list` use case emerges",
            },
        ),
        (
            "/api/v1/webhook-deliveries/{delivery_id}",
            Covered {
                cli_path: &["webhook-deliveries", "get"],
                mcp_tool: "canary_webhook_delivery_get",
            },
        ),
        (
            "/api/v1/status",
            Covered {
                cli_path: &["services"],
                mcp_tool: "canary_services",
            },
        ),
        (
            "/api/v1/health-status",
            Allowlisted {
                reason: "distinct per-target/monitor state feed with no dedicated CLI/MCP surface yet; genuine gap, out of this ticket's error/webhook-delivery drill-down scope, flagged as a canary-932 follow-up",
            },
        ),
        (
            "/api/v1/targets/{id}/checks",
            Allowlisted {
                reason: "per-target probe-check history drill-down; genuine gap, out of this ticket's scope, flagged as a canary-932 follow-up",
            },
        ),
        (
            "/api/v1/incidents",
            Covered {
                cli_path: &["incidents", "list"],
                mcp_tool: "canary_incidents",
            },
        ),
        (
            "/api/v1/incidents/{id}",
            Covered {
                cli_path: &["incidents", "get"],
                mcp_tool: "canary_incident_get",
            },
        ),
        (
            "/api/v1/incidents/{incident_id}/annotations",
            Allowlisted {
                reason: "functionally superseded by the generic `/api/v1/annotations?subject_type=incident&subject_id=...` route, already covered by `canary annotations list`/`canary_annotations_list`",
            },
        ),
        (
            "/api/v1/groups/{group_hash}/annotations",
            Allowlisted {
                reason: "functionally superseded by the generic `/api/v1/annotations?subject_type=error_group&subject_id=...` route, already covered by `canary annotations list`/`canary_annotations_list`",
            },
        ),
        (
            "/api/v1/annotations",
            Covered {
                cli_path: &["annotations", "list"],
                mcp_tool: "canary_annotations_list",
            },
        ),
        (
            "/api/v1/targets",
            Covered {
                cli_path: &["targets"],
                mcp_tool: "canary_targets",
            },
        ),
        (
            "/api/v1/monitors",
            Covered {
                cli_path: &["monitors"],
                mcp_tool: "canary_monitors",
            },
        ),
        (
            "/api/v1/webhooks",
            Allowlisted {
                reason: "admin-only webhook subscription listing, used internally by doctor/integration-status probes; no standalone agent read-model use case identified yet",
            },
        ),
        (
            "/api/v1/keys",
            Allowlisted {
                reason: "admin-only API key listing; key management is an operator concern kept out of the agent coordination-loop surface by design",
            },
        ),
        (
            "/api/v1/claims",
            Covered {
                cli_path: &["claims", "list"],
                mcp_tool: "canary_claims_list",
            },
        ),
        (
            "/api/v1/claims/active",
            Covered {
                cli_path: &["claims", "active"],
                mcp_tool: "canary_claims_active",
            },
        ),
        (
            "/api/v1/claims/{id}",
            Covered {
                cli_path: &["claims", "get"],
                mcp_tool: "canary_claim_get",
            },
        ),
    ]
}

#[test]
fn every_get_operation_has_cli_and_mcp_coverage_or_a_justified_allowlist_entry()
-> Result<(), Box<dyn std::error::Error>> {
    let document: Value = serde_json::from_str(OPENAPI_JSON)?;
    let paths = document["paths"]
        .as_object()
        .ok_or_else(|| std::io::Error::other("openapi document has no paths object"))?;
    let table = parity_table();

    // Every checked-in GET path must have exactly one table entry -- this is
    // the mechanism that fails CI the moment a route is added without an
    // accounting decision.
    let table_keys: BTreeSet<&str> = table.iter().map(|(path, _)| *path).collect();
    let get_paths: BTreeSet<&str> = paths
        .iter()
        .filter(|(_, methods)| methods.get("get").is_some())
        .map(|(path, _)| path.as_str())
        .collect();

    let missing_from_table: Vec<&str> = get_paths.difference(&table_keys).copied().collect();
    assert!(
        missing_from_table.is_empty(),
        "GET operations added to priv/openapi/openapi.json with no parity_table() entry in \
         crates/canary-cli/tests/contract_parity.rs -- add a Covered{{cli_path, mcp_tool}} entry \
         or an Allowlisted{{reason}} entry for: {missing_from_table:?}"
    );

    let stale_in_table: Vec<&str> = table_keys.difference(&get_paths).copied().collect();
    assert!(
        stale_in_table.is_empty(),
        "parity_table() entries reference GET operations no longer in priv/openapi/openapi.json; \
         remove the stale entries: {stale_in_table:?}"
    );

    let mcp_tool_names: BTreeSet<&str> =
        tool_manifest().into_iter().map(|tool| tool.name).collect();

    let mut gaps = Vec::new();
    for (path, coverage) in &table {
        match coverage {
            Coverage::Allowlisted { reason } => {
                assert!(
                    !reason.trim().is_empty(),
                    "{path}: allowlist entry needs a non-empty one-line justification"
                );
            }
            Coverage::Covered { cli_path, mcp_tool } => {
                if !mcp_tool_names.contains(mcp_tool) {
                    gaps.push(format!(
                        "{path}: MCP tool `{mcp_tool}` is not registered in tool_manifest()"
                    ));
                }
                if !cli_subcommand_registered(cli_path)? {
                    gaps.push(format!(
                        "{path}: CLI command `canary {}` is not registered",
                        cli_path.join(" ")
                    ));
                }
            }
        }
    }
    assert!(
        gaps.is_empty(),
        "contract-parity guard found gaps: {gaps:#?}"
    );

    Ok(())
}

/// Prove a CLI subcommand path is live by asking the real compiled binary
/// for its `--help`. Clap resolves `--help` before enforcing required
/// positionals/subcommands, so this succeeds for any registered path and
/// fails (nonzero exit) the moment a subcommand is renamed or removed.
fn cli_subcommand_registered(cli_path: &[&str]) -> Result<bool, Box<dyn std::error::Error>> {
    let mut args: Vec<&str> = cli_path.to_vec();
    args.push("--help");
    let output = Command::new(env!("CARGO_BIN_EXE_canary"))
        .args(&args)
        .output()?;
    Ok(output.status.success())
}

// --- Emitted-command-grammar regression -----------------------------------
//
// Each test below drives one real payload-building function with the
// smallest input that reaches its embedded `canary ...` command string, then
// proves that string both contains the expected grammar and resolves as a
// live CLI subcommand.

#[test]
fn integration_plan_verify_command_uses_registered_cli_grammar()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project_dir("plan-verify")?;
    fs::write(root.join("index.html"), "<h1>static</h1>")?;

    let plan = integration_plan(&IntegrationInput {
        target: root.clone(),
        service: Some("qa-smoke".to_owned()),
        production_url: Some("https://qa-smoke.example.com".to_owned()),
        platform_project: None,
        endpoint: DEFAULT_ENDPOINT.to_owned(),
    })?;
    let verify = plan["commands"]["verify"]
        .as_str()
        .ok_or("missing commands.verify")?;
    assert_command_uses_registered_cli_grammar(verify, "canary errors list ", &["errors", "list"])?;

    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn integration_patch_next_steps_and_receipt_use_registered_cli_grammar()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project_dir("patch-next-steps")?;
    fs::write(
        root.join("package.json"),
        r#"{"dependencies":{"next":"15.0.0"}}"#,
    )?;

    let patched = integration_patch(&IntegrationInput {
        target: root.clone(),
        service: Some("qa-smoke".to_owned()),
        production_url: Some("https://qa-smoke.example.com".to_owned()),
        platform_project: None,
        endpoint: DEFAULT_ENDPOINT.to_owned(),
    })?;

    let next_steps = patched["next_steps"]
        .as_array()
        .ok_or("missing next_steps")?;
    let deploy_step = next_steps
        .iter()
        .filter_map(Value::as_str)
        .find(|step| step.contains("canary errors"))
        .ok_or("no next_steps entry mentions canary errors")?;
    assert_command_uses_registered_cli_grammar(
        deploy_step,
        "canary errors list <service>",
        &["errors", "list"],
    )?;

    // `integration_patch` also writes the on-disk `.canary/integration.json`
    // receipt with its own independently-built verification_commands.
    let receipt: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".canary/integration.json"))?)?;
    let verification_commands = receipt["verification_commands"]
        .as_array()
        .ok_or("missing verification_commands")?;
    let errors_command = verification_commands
        .iter()
        .filter_map(Value::as_str)
        .find(|command| command.contains("canary errors"))
        .ok_or("no verification_commands entry mentions canary errors")?;
    assert_command_uses_registered_cli_grammar(
        errors_command,
        "canary errors list ",
        &["errors", "list"],
    )?;

    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn cli_integrate_patch_emits_canonical_adapter_files() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project_dir("patch-process")?;
    fs::write(
        root.join("package.json"),
        r#"{"dependencies":{"next":"15.0.0"}}"#,
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_canary"))
        .args([
            "integrate",
            "patch",
            root.to_str().ok_or("invalid temp project path")?,
            "--service",
            "qa-smoke",
            "--endpoint",
            DEFAULT_ENDPOINT,
            "--production-url",
            "https://qa-smoke.example.com",
            "--json",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "integrate patch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(payload["response"]["schema_version"], json!(2));

    let adapter = fs::read_to_string(root.join("canary.ts"))?;
    let instrumentation = fs::read_to_string(root.join("instrumentation.ts"))?;
    assert!(adapter.contains("Generated by canary integrate patch"));
    assert!(adapter.contains("scrubContext"));
    assert!(instrumentation.contains("requestInfo.path"));
    assert!(instrumentation.contains("requestInfo.method"));

    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn integration_enroll_receipt_verification_commands_use_registered_cli_grammar()
-> Result<(), Box<dyn std::error::Error>> {
    let server = FixtureServer::spawn(vec![FixtureResponse::created(json!({
        "target": {"id": "TGT-qa-smoke"},
        "api_key": {"id": "KEY-qa-smoke", "key": "sk_live_redacted_for_test"}
    }))])?;
    let root = temp_project_dir("enroll-receipt")?;
    let client = ApiClient::new(Config::resolve(
        Some(server.endpoint().to_owned()),
        Some("admin-key".to_owned()),
        None,
    )?)?;

    integration_enroll(
        &client,
        &IntegrationEnrollRequest {
            service: "qa-smoke".to_owned(),
            url: "https://qa-smoke.example.com/api/health".to_owned(),
            environment: "production".to_owned(),
            interval_ms: None,
            redact: true,
            receipt_root: Some(root.clone()),
        },
    )?;
    let requests = server.join()?;
    assert_eq!(requests.len(), 1);

    let receipt: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".canary/integration.json"))?)?;
    let verification_commands = receipt["verification_commands"]
        .as_array()
        .ok_or("missing verification_commands")?;
    let errors_command = verification_commands
        .iter()
        .filter_map(Value::as_str)
        .find(|command| command.contains("canary errors"))
        .ok_or("no verification_commands entry mentions canary errors")?;
    assert_command_uses_registered_cli_grammar(
        errors_command,
        "canary errors list ",
        &["errors", "list"],
    )?;

    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn doctor_next_operator_action_error_hint_uses_registered_cli_grammar()
-> Result<(), Box<dyn std::error::Error>> {
    let witness = json!({"status": "observed", "state": "up"});
    let hint = next_operator_action("degraded", &witness, 0, 0, 3, 0, None);
    assert_command_uses_registered_cli_grammar(
        &hint,
        "canary errors list canary --window 1h --json",
        &["errors", "list"],
    )
}

/// Assert one emitted command string both contains the expected grammar and
/// resolves as a live CLI subcommand -- the latter check is what makes this
/// structurally resistant to renaming the subcommand out from under a
/// hardcoded string comparison.
fn assert_command_uses_registered_cli_grammar(
    text: &str,
    needle: &str,
    cli_path: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        text.contains(needle),
        "expected {text:?} to contain {needle:?}"
    );
    assert!(
        cli_subcommand_registered(cli_path)?,
        "canary {} is not a registered CLI subcommand",
        cli_path.join(" ")
    );
    Ok(())
}

fn temp_project_dir(name: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let root = std::env::temp_dir().join(format!("canary-cli-contract-parity-{name}-{nonce}"));
    fs::create_dir_all(&root)?;
    Ok(root)
}
