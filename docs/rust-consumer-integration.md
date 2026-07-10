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
| `CANARY_ENDPOINT` | Base URL, e.g. `https://canary.mistystep.io`. Trailing `/` trimmed. | Reporter no-ops. |
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

/// Like `report_error`, but sends synchronously on the CALLING thread — for the
/// panic hook, where an uncaught panic can exit the process before a detached
/// `spawn_send` thread finishes. Same env-gating and swallow-everything contract.
pub fn report_error_blocking(error_class: &str, message: &str) {
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
    send_once(&endpoint, &key, "/api/v1/errors", &body);
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

/// The actual send: short-timeout agent, POST once with one retry, all failures
/// swallowed. Shared by the off-thread `spawn_send` and the inline
/// `report_error_blocking`.
fn send_once(endpoint: &str, key: &str, path: &str, body: &serde_json::Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(SEND_TIMEOUT))
        .build()
        .into();
    let url = format!("{endpoint}{path}");
    let auth = format!("Bearer {key}");
    for _ in 0..2 {
        // one retry, then give up silently
        if agent.post(&url).header("Authorization", &auth).send_json(body).is_ok() {
            break;
        }
    }
}

fn spawn_send(endpoint: String, key: String, path: &'static str, body: serde_json::Value) {
    // Detached, best-effort: bound in-flight sends in a real reporter (a shared
    // client + a semaphore) so an ERROR burst can't spawn threads without limit.
    let _ = std::thread::Builder::new()
        .name("canary-report".into())
        .spawn(move || send_once(&endpoint, &key, path, &body));
}
```

> Zero-dependency variant: if you cannot add `ureq`, replicate
> `bitterblossom/src/canary.rs` — it spawns `curl --config -` and passes the
> `Authorization` header + body on stdin so the secret never lands in `argv`.
> Same JSON bodies, same swallow-everything contract.

## Wiring points

- **Standing services** (glass, mint, cairn, roster-api, roster-mcp,
  memory-engine-api, bitterblossom; also the long-running *modes* of
  otherwise-CLI binaries: `crucible serve`/`mcp`, `doomscrum serve`,
  `glance-next serve-local`, `cairn mcp`): install the panic hook and the
  tracing→Canary layer once at process start, call
  `canary::start_health_loop()` **in every long-running bootstrap** (each
  `serve`/`mcp`/daemon entry, not just `main`), and report at the service
  boundary (Axum 5xx mapping, MCP tool error arm, detached-task error arm).
  A one-shot `check_in()` is **not enough** for a process that outlives the
  TTL — it goes falsely overdue. See **Comprehensive coverage** below.
- **CLIs / build tools** (`roster`, `glance-next`, `crucible`, `doomscrum`
  one-shot subcommands): install the panic hook + tracing layer, call
  `canary::check_in()` once per run, and rely on the tracing layer +
  top-level `report_error` for errors. No background loop for the one-shot
  path — overdue between runs is expected, not an incident. **But if the same
  binary has a `serve`/`mcp` mode, that mode is a standing service** and needs
  `start_health_loop()`.

> **A single top-level `report_error` at `main()`'s `Err` arm is the shallow
> pattern** — it only sees errors that *propagate* out of `main`. It is blind
> to panics, request-handler errors, errors inside `tokio::spawn` tasks, and
> anything logged-and-swallowed. The comprehensive pattern below makes error
> capture a property of **logging** (every `error!` is reported) plus a panic
> hook, so you stop hand-wiring call sites.

## Comprehensive coverage (services, panics, app logging)

Adoption is not "one fired event" — it is *every* error path and *continuous*
health for every standing service. Four additions turn the shallow reporter
into comprehensive coverage:

### 1. Auto-capture every `ERROR` log (the high-leverage move)

A `tracing` layer that forwards every `ERROR`-level event to `report_error`.
Now "app logging" **is** error capture: any `tracing::error!(...)` anywhere in
the app (or its libraries) lands in Canary with zero per-site wiring. Migrate
raw `eprintln!`/`log::error!` error sites to `tracing::error!` (bridge
`log`-only crates with `tracing_log::LogTracer::init()`).

```rust
// in canary.rs — deps: tracing, tracing-subscriber
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

pub struct CanaryLayer;

impl<S: Subscriber> Layer<S> for CanaryLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if config().is_none() || *event.metadata().level() != Level::ERROR { return; }
        let mut msg = String::new();
        event.record(&mut Visitor(&mut msg));           // pulls `message` + fields
        let class = format!("{}.{}", service(), event.metadata().target());
        report_error(&class, &redact(&msg));            // redact() = identity unless secret-sensitive
    }
}

struct Visitor<'a>(&'a mut String);
impl tracing::field::Visit for Visitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if !self.0.is_empty() { self.0.push(' '); }
        self.0.push_str(&format!("{}={:?}", field.name(), value));
    }
}
```

Register it with a **per-layer** filter, never behind a global one:
```rust
use tracing_subscriber::{prelude::*, EnvFilter, filter::LevelFilter};
tracing_subscriber::registry()
    .with(fmt_layer.with_filter(EnvFilter::from_default_env()))   // RUST_LOG affects the CONSOLE only
    .with(canary::CanaryLayer.with_filter(LevelFilter::ERROR))    // Canary sees EVERY error, always
    .init();
```

> **GOTCHA — filter swallow (bit cairn live, 2026-07-07):** do NOT write
> `registry().with(fmt_layer).with(EnvFilter).with(CanaryLayer)` or
> `Subscriber::builder()...finish().with(CanaryLayer)`. A **global** `EnvFilter`
> (or the fmt subscriber's built-in filter) runs *before* the layer, so at any
> realistic `RUST_LOG` (e.g. `myapp=info`) every ERROR whose target isn't your
> crate — `tower_http::catch_panic`, `tower_http::trace::on_failure`, a
> dependency's error — is dropped before `CanaryLayer` ever sees it. Attach the
> `EnvFilter` **per-layer to the fmt layer** and give `CanaryLayer` its own
> `LevelFilter::ERROR`. Verify by firing an error at `RUST_LOG=off` and
> confirming it still lands at the hub.

> **GOTCHA — self-recursion:** because `CanaryLayer` is unfiltered, a failed
> send that itself logs `error!` (via `hyper`/`reqwest`/`rustls`/`h2`) would
> re-enter the layer. Deny those transport targets in `on_event` (early-return
> if `event.metadata().target()` starts with a transport crate).

> **Secret-sensitive apps (mint):** the layer is a new leak surface. `redact()`
> must scrub the message/fields to failure *shape* (op name, policy id, upstream
> status) before sending — never a credential, token, or request/response body.
> A redaction test on the auto-forwarded path is mandatory.

### 2. Capture panics

```rust
pub fn install_panic_hook() {
    if config().is_none() { return; }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info.payload().downcast_ref::<&str>().map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_owned());
        report_error_blocking(&format!("{}.panic", service()), &format!("{msg} @ {loc}"));
        default(info);
    }));
}
```

> **GOTCHA — panic send must be BLOCKING (bit cairn live, 2026-07-07):** if your
> reporter sends via `tokio::spawn` (async reporters using `reqwest`), the spawn
> is **cancelled when the runtime drops during process teardown**, so an uncaught
> panic's report never leaves the box. The panic hook must send **synchronously**
> — its own `std::thread` + a bounded current-thread runtime (or a blocking HTTP
> client like `ureq`) — and complete before returning to the default hook. This
> is why the module above splits `report_error` (detached `spawn_send`, for hot
> paths) from `report_error_blocking` (inline `send_once`): **even the `ureq`
> module's `report_error` detaches its send**, so it can be lost if the process
> is already unwinding. The panic hook must call `report_error_blocking`. Prove
> it with a test that triggers the hook with **no ambient tokio runtime** and
> asserts the POST lands.

For Axum, also add `tower_http::catch_panic::CatchPanicLayer::new(...)` so a
panicking handler **reports and returns 500** instead of silently killing the
worker task. (For a *caught* handler panic the process survives, so a
fire-and-forget send has time to land; the blocking requirement above is for the
*uncaught* panic that ends the process.)

> **GOTCHA — double-reported panics (found in review, 2026-07-07):** `CatchPanicLayer`
> emits its OWN `tracing::error!` at target `tower_http::catch_panic`, and your
> `install_panic_hook` *also* fires on the same unwind — so one caught panic
> lands **twice** (`<app>.panic` and `<app>.tower_http::catch_panic`). The panic
> hook already reports every panic authoritatively, so add `tower_http::catch_panic`
> to the `CanaryLayer` denied-target prefixes (§1) — but keep other `tower_http`
> targets like `trace::on_failure`, which are legitimate 5xx signals.

### 3. Continuous health in *every* standing-service bootstrap

`start_health_loop()` fires once immediately, then every 60s from a named
thread (TTL 120s). Call it at the top of **each** long-running entry point —
the HTTP `serve`, the MCP stdio loop, any daemon — not only in a CLI one-shot.
A process that outlives the TTL without a loop reads as `down` while perfectly
healthy (the exact bug the audit found in cairn-mcp, crucible serve/mcp,
doomscrum serve, glance-next serve-local).

### 4. Report at the service boundary

Report where a running service actually fails, not just at `main`:
Axum's error→response mapping (e.g. `impl IntoResponse for AppError`), the MCP
tool error arm, and each detached `tokio::spawn` task's error arm. Prefer
emitting `tracing::error!` at those sites so layer #1 captures them
automatically — that keeps it one declaration, not N manual calls.

## Test it without the network (mock server)

Mirror `memory-engine-canary/tests/reporter.rs`: bind a `TcpListener` on
`127.0.0.1:0`, set `CANARY_ENDPOINT`/`CANARY_API_KEY` to point at it, call the
reporter, and assert the request body. Add one test that points at a **dead
port** and asserts the call returns without panic/hang — that proves invariant
2 (an outage never reaches the caller).

> **GOTCHA — tests fire REAL events if `CANARY_*` is in your shell (fleet-wide,
> 2026-07-07):** the reporter is env-gated, so any `cargo test`/smoke run in a
> shell that already exports `CANARY_ENDPOINT` + `CANARY_API_KEY` will send real
> check-ins/errors to production under your service name — especially tests that
> spawn the real binary as a subprocess (it inherits the env). Every test that
> exercises a real run must **clear the env** (`Command::env_remove(...)` for
> subprocesses; the reporter's own unit tests set only the mock's endpoint), and
> run the gate with `env -u CANARY_ENDPOINT -u CANARY_API_KEY -u CANARY_INGEST_KEY`.
> Tracked fleet-wide as `canary-102`.

## Fire the proof event (the wiring oracle)

"Merged the PR" is **not** wired. Adoption is proven only by a live signal at
the hub. From the app checkout, with a real ingest key in env:

```bash
CANARY_ENDPOINT=https://canary.mistystep.io \
CANARY_API_KEY=<ingest-key-from-secret-store> \
  cargo run --release -- <a normal invocation>   # or boot the service

# read back with a read-scoped key ($CANARY_READ_API_KEY — inspection only, not
# used by the reporter). Errors surface as ERROR GROUPS + correlated incidents,
# NOT a flat `.errors` array:
curl -fsS -H "Authorization: Bearer $CANARY_READ_API_KEY" \
  "https://canary.mistystep.io/api/v1/report?window=1h" \
  | jq '.monitors[]     | select(.service=="<service>"),
        .error_groups[]? | select(.service=="<service>")'
```

The app is integrated only when the readback shows the monitor's
`last_check_in_at` moving and/or the error landing under the service.

## Per-repo checklist

**Baseline reporter**
- [ ] `src/canary.rs` (or `<app>-canary` crate) added, gated on env, no-ops without creds.
- [ ] Deps: `serde_json` + `ureq` (or zero-dep `curl` variant).
- [ ] Monitor exists in Canary (name = your `MONITOR` const). Provisioned out-of-band.
- [ ] Mock-server unit test + dead-port no-hang test.

**Comprehensive coverage (required — the shallow single-call pattern is not enough)**
- [ ] `install_panic_hook()` called once at process start; a forced panic reports `<app>.panic` at the hub.
- [ ] `CanaryLayer` registered in the tracing subscriber; a `tracing::error!` anywhere is observed at the hub. Raw `eprintln!`/`log::error!` error sites migrated to `tracing::error!` (or `LogTracer` bridged).
- [ ] **Every** long-running mode (`serve`/`mcp`/daemon) calls `start_health_loop()` — a process running past the 120s TTL stays `up`, not falsely overdue.
- [ ] Service-boundary reporting: Axum 5xx mapping + MCP tool error arm + detached-task error arms emit `tracing::error!` (captured by the layer) or `report_error` directly. Axum gets `CatchPanicLayer`.
- [ ] CLIs/tools: one `check_in()` per run (overdue-between-runs expected). Any `serve`/`mcp` subcommand still needs the health loop.
- [ ] Secret-sensitive apps (mint): `redact()` proven on the auto-forwarded tracing path — failure shape only, never a credential/token/body.

**Proof + gate**
- [ ] `cargo build` / gate green; no gate lowered, no error silenced to look green.
- [ ] **Fired-event proof (comprehensive)**: for a service — monitor STAYS up while the process runs; a forced handler/task error AND a forced panic observed at the hub. For a CLI — check-in + a forced error observed. Record IDs.
</content>
</invoke>
