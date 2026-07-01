//! Stdio MCP smoke tests for the Canary CLI adapter.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{Value, json};

#[test]
fn mcp_stdio_lists_and_calls_cli_backed_tools() -> Result<(), Box<dyn std::error::Error>> {
    let repo_root = repo_root()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_canary"))
        .arg("mcp-server")
        .current_dir(&repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("child stdin unavailable"))?;
    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "canary-test", "version": "0"}
            }
        })
    )?;
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )?;
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    )?;
    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "canary_integrate_discover",
                "arguments": {"path_or_project": "."}
            }
        })
    )?;
    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "canary_not_a_tool",
                "arguments": {}
            }
        })
    )?;
    drop(stdin);

    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let responses = String::from_utf8(output.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        responses.len(),
        4,
        "initialized notification has no response"
    );

    assert_eq!(responses[0]["id"], json!(1));
    assert_eq!(
        responses[0]["result"]["protocolVersion"],
        json!("2025-11-25")
    );
    assert_eq!(
        responses[0]["result"]["capabilities"]["tools"]["listChanged"],
        json!(false)
    );

    let tools = responses[1]["result"]["tools"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("tools/list did not return a tools array"))?;
    let discover = tools
        .iter()
        .find(|tool| tool["name"] == "canary_integrate_discover")
        .ok_or_else(|| std::io::Error::other("discover tool not listed"))?;
    assert!(discover.get("input_schema").is_none());
    assert_eq!(discover["inputSchema"]["type"], json!("object"));

    assert_eq!(responses[2]["id"], json!(3));
    assert_eq!(responses[2]["result"]["isError"], Value::Null);
    assert_eq!(responses[2]["result"]["content"][0]["type"], json!("text"));
    assert_eq!(
        responses[2]["result"]["structuredContent"]["command"],
        json!("canary_integrate_discover")
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["response"]["schema_version"],
        json!(1)
    );
    assert_eq!(responses[3]["id"], json!(4));
    assert_eq!(responses[3]["result"]["isError"], json!(true));
    assert_eq!(
        responses[3]["result"]["structuredContent"]["error"]["message"],
        json!("unknown Canary MCP tool: canary_not_a_tool")
    );

    Ok(())
}

fn repo_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .ok_or_else(|| std::io::Error::other("repo root not found").into())
}
