//! Stdio MCP smoke tests for the Canary CLI adapter.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
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

#[test]
fn cli_incidents_get_reads_incident_detail() -> Result<(), Box<dyn std::error::Error>> {
    let server = FixtureServer::spawn(vec![FixtureResponse::ok(incident_detail_body())])?;
    let output = Command::new(env!("CARGO_BIN_EXE_canary"))
        .args([
            "--endpoint",
            server.endpoint(),
            "--api-key",
            "read-key",
            "--json",
            "incidents",
            "get",
            "INC-loop",
        ])
        .output()?;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["command"], json!("incidents get"));
    assert_eq!(response["response"]["incident"]["id"], json!("INC-loop"));

    let requests = server.join()?;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/api/v1/incidents/INC-loop");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer read-key")
    );

    Ok(())
}

#[test]
fn mcp_stdio_exercises_incident_loop_tools() -> Result<(), Box<dyn std::error::Error>> {
    let server = FixtureServer::spawn(vec![
        FixtureResponse::ok(incident_detail_body()),
        FixtureResponse::created(claim_body("claimed")),
        FixtureResponse::created(annotation_body()),
        FixtureResponse::ok(claim_body("released")),
    ])?;
    let repo_root = repo_root()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_canary"))
        .args(["--endpoint", server.endpoint(), "mcp-server"])
        .current_dir(&repo_root)
        .env("CANARY_RESPONDER_KEY", "mcp-key")
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
                "clientInfo": {"name": "canary-loop-test", "version": "0"}
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
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "canary_incident_get",
                "arguments": {"incident_id": "INC-loop"}
            }
        })
    )?;
    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "canary_claim_create",
                "arguments": {
                    "subject_type": "incident",
                    "subject_id": "INC-loop",
                    "owner": "codex",
                    "purpose": "triage",
                    "ttl_ms": 900000,
                    "idempotency_key": "run-loop"
                }
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
                "name": "canary_annotation_create",
                "arguments": {
                    "subject_type": "incident",
                    "subject_id": "INC-loop",
                    "agent": "codex",
                    "action": "fix-verified",
                    "metadata": {
                        "claim_id": "CLM-loop",
                        "evidence": "https://example.com/proof"
                    }
                }
            }
        })
    )?;
    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "canary_claim_release",
                "arguments": {
                    "claim_id": "CLM-loop",
                    "owner": "codex"
                }
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
    assert_eq!(responses.len(), 5);
    assert_eq!(
        responses[1]["result"]["structuredContent"]["command"],
        json!("canary_incident_get")
    );
    assert_eq!(
        responses[1]["result"]["structuredContent"]["response"]["incident"]["id"],
        json!("INC-loop")
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["response"]["state"],
        json!("claimed")
    );
    assert_eq!(
        responses[3]["result"]["structuredContent"]["response"]["action"],
        json!("fix-verified")
    );
    assert_eq!(
        responses[4]["result"]["structuredContent"]["response"]["state"],
        json!("released")
    );

    let requests = server.join()?;
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str()))
            .collect::<Vec<_>>(),
        [
            ("GET", "/api/v1/incidents/INC-loop"),
            ("POST", "/api/v1/claims"),
            ("POST", "/api/v1/annotations"),
            ("POST", "/api/v1/claims/CLM-loop/release")
        ]
    );
    assert!(
        requests
            .iter()
            .all(|request| request.authorization.as_deref() == Some("Bearer mcp-key"))
    );
    assert!(requests[1].body.contains("\"subject_type\":\"incident\""));
    assert!(requests[2].body.contains("\"action\":\"fix-verified\""));

    Ok(())
}

fn repo_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .ok_or_else(|| std::io::Error::other("repo root not found").into())
}

#[derive(Debug)]
struct FixtureServer {
    endpoint: String,
    handle: thread::JoinHandle<Result<Vec<RecordedRequest>, String>>,
}

impl FixtureServer {
    fn spawn(responses: Vec<FixtureResponse>) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || serve_fixture(listener, responses));
        Ok(Self { endpoint, handle })
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn join(self) -> Result<Vec<RecordedRequest>, Box<dyn std::error::Error>> {
        let result = self
            .handle
            .join()
            .map_err(|_| std::io::Error::other("fixture server thread failed"))?;
        result.map_err(|message| std::io::Error::other(message).into())
    }
}

#[derive(Debug, Clone)]
struct FixtureResponse {
    status: u16,
    body: Value,
}

impl FixtureResponse {
    fn ok(body: Value) -> Self {
        Self { status: 200, body }
    }

    fn created(body: Value) -> Self {
        Self { status: 201, body }
    }
}

#[derive(Debug)]
struct RecordedRequest {
    method: String,
    path: String,
    authorization: Option<String>,
    body: String,
}

fn serve_fixture(
    listener: TcpListener,
    responses: Vec<FixtureResponse>,
) -> Result<Vec<RecordedRequest>, String> {
    let mut requests = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    for response in responses {
        loop {
            match listener.accept() {
                Ok((mut stream, _addr)) => {
                    let request = read_request(&mut stream).map_err(|error| error.to_string())?;
                    write_response(&mut stream, response).map_err(|error| error.to_string())?;
                    requests.push(request);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        return Err(format!(
                            "timed out waiting for request {}",
                            requests.len() + 1
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    Ok(requests)
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<RecordedRequest> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut bytes = Vec::new();
    let mut header_end = None;
    let mut content_length = 0;
    loop {
        let mut buffer = [0_u8; 1024];
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if header_end.is_none()
            && let Some(position) = find_header_end(&bytes)
        {
            content_length = parse_content_length(&bytes[..position]);
            header_end = Some(position);
        }
        if let Some(position) = header_end
            && bytes.len() >= position + 4 + content_length
        {
            break;
        }
    }

    let text = String::from_utf8_lossy(&bytes).to_string();
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_owned();
    let path = request_parts.next().unwrap_or_default().to_owned();
    let authorization = text
        .split("\r\n")
        .find_map(|line| {
            line.strip_prefix("authorization: ")
                .or_else(|| line.strip_prefix("Authorization: "))
        })
        .map(str::to_owned);
    let body = header_end
        .and_then(|position| bytes.get(position + 4..))
        .map(|body| String::from_utf8_lossy(body).to_string())
        .unwrap_or_default();

    Ok(RecordedRequest {
        method,
        path,
        authorization,
        body,
    })
}

fn write_response(stream: &mut TcpStream, response: FixtureResponse) -> std::io::Result<()> {
    let body = response.body.to_string();
    let reason = match response.status {
        200 => "OK",
        201 => "Created",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        response.status,
        reason,
        body.len(),
        body
    )
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(headers: &[u8]) -> usize {
    String::from_utf8_lossy(headers)
        .split("\r\n")
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

fn incident_detail_body() -> Value {
    json!({
        "summary": "incident INC-loop: api incident",
        "incident": {
            "id": "INC-loop",
            "service": "api",
            "state": "investigating",
            "severity": "medium",
            "title": "api incident",
            "opened_at": "2026-05-28T20:00:00Z",
            "resolved_at": null,
            "signal_count": 1
        },
        "signals": [],
        "signals_truncated": false,
        "annotations": [],
        "annotations_truncated": false,
        "claims": [],
        "recent_timeline_events": []
    })
}

fn claim_body(state: &str) -> Value {
    json!({
        "id": "CLM-loop",
        "tenant_id": "TENANT-bootstrap",
        "project_id": "PROJECT-bootstrap",
        "service": "api",
        "subject_type": "incident",
        "subject_id": "INC-loop",
        "owner": "codex",
        "purpose": "triage",
        "state": state,
        "idempotency_key": "run-loop",
        "evidence_links": [],
        "created_at": "2026-05-28T20:01:00Z",
        "updated_at": "2026-05-28T20:02:00Z",
        "expires_at": "2026-05-28T20:16:00Z",
        "released_at": if state == "released" { json!("2026-05-28T20:02:00Z") } else { Value::Null },
        "completed_at": if state == "released" { json!("2026-05-28T20:02:00Z") } else { Value::Null }
    })
}

fn annotation_body() -> Value {
    json!({
        "id": "ANN-loop",
        "subject_type": "incident",
        "subject_id": "INC-loop",
        "incident_id": "INC-loop",
        "group_hash": null,
        "agent": "codex",
        "action": "fix-verified",
        "metadata": {
            "claim_id": "CLM-loop",
            "evidence": "https://example.com/proof"
        },
        "created_at": "2026-05-28T20:03:00Z"
    })
}
