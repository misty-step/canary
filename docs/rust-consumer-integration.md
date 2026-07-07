# Rust In-Process Canary Integration

The 15-minute path for a **Rust** app, service, worker, CLI, or build tool to
report its own errors and health check-ins into Canary from inside its own
process. This is the in-process counterpart to `factory-fleet-integration.md`
(which is the operator-side curl enrollment path for URL-polled HTTP targets).

Use this when the thing you want observed is a Rust binary that runs (a service
loop, a worker, a CLI invocation, a scheduled build) — not just a public health
URL Canary can poll from outside.

The fleet already runs two proven reference implementations. This doc distills
their convergence into one copy-paste pattern:

- `memory-engine/crates/memory-engine-canary` — a real in-process `ureq`
  reporter with a background check-in loop and bounded, swallowed sends.
- `bitterblossom/src/canary.rs` — the same contract with **zero** HTTP
  dependencies, shelling out to system `curl` with the secret on stdin.

## Invariants (the contract every consumer honors)

1. **Gate on env; missing creds ⇒ silent no-op.** The reporter reads
   `CANARY_ENDPOINT` and `CANARY_API_KEY` (accept `CANARY_INGEST_KEY` as an
   alias for older repos). If either is unset or empty, every call is a no-op.
   No panics, no logs on the hot path, no behavior change for local dev.
2. **A Canary outage never touches the app.** Sends run off the request/worker
   path (a background thread or a child process), with a short timeout (2–10s)
   and at most one retry. Every failure is swallowed. Bound in-flight sends so a
   slow hub can't leak threads.
3. **The app never creates its monitor.** `POST /api/v1/monitors` is
   provisioned out-of-band (operator/enrollment sweep). The app only sends
   check-ins to a monitor name that already exists.
4. **Auth is `Authorization: Bearer <key>`, `Content-Type: application/json`.**
   Use a scoped **ingest-only** key in steady state, never an admin key.
5. **Errors are captured at explicit failure sites** (`report_error(class,
   msg)`) plus a `catch_unwind` around request/worker boundaries. No global
   panic hook, no tracing subscriber requirement.
6. **Check-in = one at startup, then a named background thread every 60s** for
   services; **one per invocation** for CLIs/build tools. `ttl_ms` = 2× the
   interval (120000 for a 60s loop).

## Env contract

| Var | Meaning | Unset behavior |
|---|---|---|
| `CANARY_ENDPOINT` | Base URL, e.g. `https://canary-obs.fly.dev`. Trailing `/` trimmed. | Reporter no-ops. |
| `CANARY_API_KEY` | Ingest-scoped bearer key. (`CANARY_INGEST_KEY` accepted as alias.) | Reporter no-ops. |
| `CANARY_SERVICE` | Optional override for the reported `service` name. | Falls back to the module const. |
| `CANARY_ENVIRONMENT` | Optional `environment` tag. | Defaults to `"production"`. |

**Never read the key file or print the key.** In steady state the value comes
from the app's own secret store / Fly secret. For a one-time fired-event proof
the operator supplies it via env from a sanctioned key file, never inlined.

## HTTP surface used

- `POST {endpoint}/api/v1/errors` — body:
  ```json
  { "service": "...", "error_class": "...", "message": "...",
    "severity": "error", "environment": "production",
    "context": { }, "fingerprint": ["..."] }
  ```
  Required: `service`, `error_class`, `message`. `message` ≤ 4096 chars.
- `POST {endpoint}/api/v1/check-ins` — body:
  ```json
  { "monitor": "...", "status": "alive", "summary": "...", "ttl_ms": 120000 }
  ```
  Required: `monitor`, `status` (`alive` | `in_progress` | `ok` | `error`).

## The reporter module (copy this)

Drop this in as `src/canary.rs` (or a small `<app>-canary` crate for
workspaces). It adds two deps: `serde_json` and `ureq`. Swap `SERVICE` and
`MONITOR` for your names, then wire the three public fns.

```rust
//! Fire-and-forget Canary self-reporter. No creds => silent no-op.
//! A Canary outage never blocks, slows, or panics the host app.
use std::time::Duration;

const SERVICE: &str = "<your-service>"; // overridable via CANARY_SERVICE
const MONITOR: &str = "<your-monitor>"; // must already exist in Canary
const CHECKIN_INTERVAL: Duration = Duration::from_secs(60);
const TTL_MS: u64 = 120_000;
const SEND_TIMEOUT: Duration = Duration::from_secs(3);

fn config() -> Option<(String, String)> {
    let endpoint = std::env::var("CANARY_ENDPOINT").ok()?;
    let key = std::env::var("CANARY_API_KEY")
        .or_else(|_| std::env::var("CANARY_INGEST_KEY"))
        .ok()?;
    (!endpoint.trim().is_empty() && !key.trim().is_empty())
        .then(|| (endpoint.trim_end_matches('/').to_owned(), key))
}

fn service() -> String {
    std::env::var("CANARY_SERVICE").ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| SERVICE.to_owned())
}

/// Report a handled or unhandled error. Safe to call anywhere.
pub fn report_error(error_class: &str, message: &str) {
    let Some((endpoint, key)) = config() else { return };
    let environment = std::env::var("CANARY_ENVIRONMENT")
        .unwrap_or_else(|_| "production".to_owned());
    let body = serde_json::json!({
        "service": service(),
        "error_class": error_class,
        "message": message.chars().take(4096).collect::<String>(),
        "severity": "error",
        "environment": environment,
    });
    spawn_send(endpoint, key, "/api/v1/errors", body);
}

/// Heartbeat: one at startup, then the background loop drives it.
pub fn check_in() {
    let Some((endpoint, key)) = config() else { return };
    let body = serde_json::json!({
        "monitor": MONITOR,
        "status": "alive",
        "summary": concat!(env!("CARGO_PKG_NAME"), " heartbeat"),
        "ttl_ms": TTL_MS,
    });
    spawn_send(endpoint, key, "/api/v1/check-ins", body);
}

/// Services only: fire once now, then every 60s from a named thread.
pub fn start_health_loop() {
    if config().is_none() { return; }
    check_in();
    let _ = std::thread::Builder::new()
        .name("canary-health".into())
        .spawn(|| loop {
            std::thread::sleep(CHECKIN_INTERVAL);
            check_in();
        });
}

fn spawn_send(endpoint: String, key: String, path: &'static str, body: serde_json::Value) {
    let _ = std::thread::Builder::new()
        .name("canary-report".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(SEND_TIMEOUT))
                .build()
                .into();
            let url = format!("{endpoint}{path}");
            let auth = format!("Bearer {key}");
            for _ in 0..2 { // one retry, then give up silently
                let ok = agent
                    .post(&url)
                    .header("Authorization", &auth)
                    .send_json(&body)
                    .is_ok();
                if ok { break }
            }
        });
}
```

> Zero-dependency variant: if you cannot add `ureq`, replicate
> `bitterblossom/src/canary.rs` — it spawns `curl --config -` and passes the
> `Authorization` header + body on stdin so the secret never lands in `argv`.
> Same JSON bodies, same swallow-everything contract.

## Wiring points

- **Services / long-running workers** (glass, mint, cairn, memory-engine-api,
  bitterblossom): call `canary::start_health_loop()` once in `main`/server
  bootstrap; call `canary::report_error(class, msg)` at your typed failure
  sites; wrap the request handler and any worker run in `catch_unwind` and
  report `"<app>.panic"` on unwind.
- **CLIs / build tools** (crucible, roster, glance-next, doomscrum): call
  `canary::check_in()` once when a run completes (`status: "ok"` on success),
  and `canary::report_error("<app>.<subcommand>.failed", &err)` on the error
  return path in `main`. No background loop — the tool isn't a standing
  service, so its monitor expects an occasional (daily) check-in; overdue
  between runs is expected, not an incident.

## Test it without the network (mock server)

Mirror `memory-engine-canary/tests/reporter.rs`: bind a `TcpListener` on
`127.0.0.1:0`, set `CANARY_ENDPOINT`/`CANARY_API_KEY` to point at it, call the
reporter, and assert the request body. Add one test that points at a **dead
port** and asserts the call returns without panic/hang — that proves invariant
2 (an outage never reaches the caller).

## Fire the proof event (the wiring oracle)

"Merged the PR" is **not** wired. Adoption is proven only by a live signal at
the hub. From the app checkout, with a real ingest key in env:

```bash
CANARY_ENDPOINT=https://canary-obs.fly.dev \
CANARY_API_KEY=<ingest-key-from-secret-store> \
  cargo run --release -- <a normal invocation>   # or boot the service

# read back — the monitor state and/or error must appear:
curl -fsS -H "Authorization: Bearer $CANARY_READ_API_KEY" \
  "https://canary-obs.fly.dev/api/v1/report?window=1h" \
  | jq '.monitors[] | select(.service=="<service>"), .errors'
```

The app is integrated only when the readback shows the monitor's
`last_check_in_at` moving and/or the error landing under the service.

## Per-repo checklist

- [ ] `src/canary.rs` (or `<app>-canary` crate) added, gated on env, no-ops without creds.
- [ ] Deps: `serde_json` + `ureq` (or zero-dep `curl` variant).
- [ ] `report_error` wired at typed failure sites + `catch_unwind` on request/worker boundaries.
- [ ] `start_health_loop()` (service) **or** one `check_in()` per run (CLI/tool).
- [ ] Monitor exists in Canary (name = your `MONITOR` const). Provisioned out-of-band.
- [ ] Mock-server unit test + dead-port no-hang test.
- [ ] `cargo build` / gate green; no gate lowered, no error silenced to look green.
- [ ] **Fired-event proof**: live check-in and/or error observed at the hub. Record IDs.
</content>
</invoke>
