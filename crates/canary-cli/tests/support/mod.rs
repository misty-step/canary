//! Shared test-only fixture HTTP server, reused across the CLI's integration
//! test binaries (`mcp_stdio.rs`, `contract_parity.rs`). Each `tests/*.rs`
//! file compiles as its own crate, so this lives under `tests/support/mod.rs`
//! (not `tests/support.rs`) to opt out of being treated as its own test
//! binary while staying importable via `mod support;`.
//!
//! Not every consumer uses every item (e.g. `contract_parity.rs` only needs
//! `FixtureResponse::created` and `RecordedRequest.len()`), so dead-code
//! warnings here are per-consumer noise, not a real unused-surface signal.
#![allow(dead_code)]

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

#[derive(Debug)]
pub struct FixtureServer {
    endpoint: String,
    handle: thread::JoinHandle<Result<Vec<RecordedRequest>, String>>,
}

impl FixtureServer {
    pub fn spawn(responses: Vec<FixtureResponse>) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || serve_fixture(listener, responses));
        Ok(Self { endpoint, handle })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn join(self) -> Result<Vec<RecordedRequest>, Box<dyn std::error::Error>> {
        let result = self
            .handle
            .join()
            .map_err(|_| std::io::Error::other("fixture server thread failed"))?;
        result.map_err(|message| std::io::Error::other(message).into())
    }
}

#[derive(Debug, Clone)]
pub struct FixtureResponse {
    status: u16,
    body: Value,
}

impl FixtureResponse {
    pub fn ok(body: Value) -> Self {
        Self { status: 200, body }
    }

    pub fn created(body: Value) -> Self {
        Self { status: 201, body }
    }
}

#[derive(Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: String,
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
