# Canary

Open-source, self-hosted observability for agent-driven infrastructure.
Ingests errors, probes health, correlates incidents, keeps timelines, and answers queries.

## Why

- Sentry's auto-issue-creation requires Business plan ($26/mo/user) — we won't pay for features we can build
- Uptime Robot is a separate surface with no agent integration
- The primary consumers of observability data are AI agents and operators, not bespoke point tools
- We want one queryable service that agents can use for incident detection, debugging, and automated response
- Existing tools (Sentry MCP, Uptime Robot API) are read-only and clunky — agents need structured, bounded, pre-aggregated data

## What It Does

1. **Error ingestion** — structured error intake via HTTP API. Replaces Sentry's core `captureException`.
2. **Health checking** — periodic HTTP probes on configurable intervals with state machine (up → degraded → down). Replaces Uptime Robot.
3. **Incident correlation + event broadcasting** — deterministic incident lifecycle plus generic webhook dispatch. Consumers define their own behavior.
4. **Queryable API** — agents query snapshots, timelines, recent errors, affected services, stack traces, and active incidents. Responses stay bounded and context-window friendly.

## What It Doesn't Do

- No workflow console, repo mutation layer, or embedded triage agent
- No session replay, performance monitoring, or user analytics
- No source map processing or release tracking (use git blame)
- No billing, team management, or RBAC
- No MCP server (API + CLI + skill files are sufficient)
- No opinionated consumer integrations (webhooks are generic; consumers wire their own behavior)
- No multi-region health checking (v1 — single region, documented as limitation)
- No high availability (v1 — single machine, restore-based DR via Litestream)

## Design Principles

- **Agent-first, operator-usable** — every API response optimized for LLM context windows. Natural-language `summary` fields. Pre-aggregated by error class. Bounded response sizes. Thin human views are fine; workflow logic stays out.
- **Single deployable service** — one Docker image, one database, one config file. No microservices, no message queues, no external dependencies beyond S3 for backups.
- **Broadcast, don't prescribe** — fire webhooks on state transitions. Don't build GitHub/Slack/Discord integrations. Let consumers decide.
- **Queryable** — structured queries for debugging: "errors in the last hour", "most frequent error class", "errors affecting service X"
- **Self-hosted** — deploy to Fly.io machines. We control the data, the uptime, the cost.
- **Open source** — MIT licensed. Useful to anyone running agent-driven infrastructure.

## Tech Stack

### Elixir/OTP + Phoenix

Architecturally ideal for this workload:
- GenServer-per-target health checkers with crash isolation via DynamicSupervisor
- Supervision tree provides automatic restart and fault tolerance
- ETS for in-memory rate limiting and deduplication caches
- **Phoenix** for HTTP routing, plug pipeline, telemetry, and a thin operator console. Pure-Plug would reimplement half of Phoenix's router and socket boundary for little gain.
- **Bandit** as HTTP server (Phoenix default since 1.7.11). Pure Elixir. HTTP/1.x up to 4x faster than Cowboy.

**Tradeoff acknowledged:** Elixir has a smaller community than Go, which limits OSS contributor pool and complicates the "single binary" narrative (BEAM release ≠ static binary). We accept this because:
1. This is primarily for our own infrastructure first
2. OTP's process model is genuinely superior for this workload (stateful periodic actors + fault isolation)
3. Our team already ships Elixir (Bitterblossom, Conductor)
4. Docker image deployment is standard for self-hosted tools

### Oban (with Lite engine)

Background job framework with native SQLite support (`Oban.Engines.Lite`). Handles **stateless, retry-oriented work** — webhook delivery, retention pruning, scheduled scans. GenServers handle stateful, cadence-sensitive work (health check probing). See "GenServer / Oban Separation" section for the full design.

| What | Owner | Why |
|------|-------|-----|
| Probe scheduling + execution | GenServer | Stateful, cadence-sensitive, needs in-memory state |
| Webhook delivery + retries | Oban | Stateless, retry-oriented, exponential backoff |
| Retention pruning | Oban | Periodic, no state needed |
| TLS expiry scan | Oban | Daily scan, no persistent state |
| Telemetry | Oban + GenServer | Both emit `:telemetry` events |

Oban manages its own tables in the same SQLite database. No external dependencies.

### SQLite (via Ecto.SQLite3)

Single-file persistence with WAL mode. `ecto_sqlite3` is built on `exqlite` (same org) — Ecto layer gives us migrations, schemas, changesets, and Oban compatibility with minimal overhead.

**Required PRAGMAs:**
```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA cache_size = -20000;     -- 20MB cache
PRAGMA foreign_keys = ON;
PRAGMA wal_autocheckpoint = 1000;
PRAGMA journal_size_limit = 67108864;  -- 64MB WAL size limit
```

**Write serialization:** Ecto repo with `pool_size: 1` for writes (SQLite is single-writer; serialization at app layer). Separate read-only repo with `pool_size: 4` for query API. Short transactions only — no network calls inside transactions.

**Why not Postgres:** Adds operational complexity (separate service, connection management, backups). SQLite is sufficient for our write volume (orders of magnitude below 10K writes/sec). Migration path to Postgres designed in via Ecto abstraction layer.

**Why not DuckDB:** Not a transactional store. Wrong tool for concurrent OLTP writes.

### Req + Finch

HTTP client for outbound health check probes.

- **Req** wraps Finch with redirect following, timeouts, automatic decompression, and response handling built in
- **Finch** provides connection pooling with per-host pools underneath (avoids TCP/TLS overhead for repeated probes to same targets)
- Shared named pool across all health checker GenServers
- Probe call: `Req.get(url, receive_timeout: 10_000, redirect: true, retry: false, finch: Canary.Finch)`

### Fly.io

Single machine deployment. Health checker runs in-process.

**v1 constraints (documented, not hidden):**
- Single region — health checks reflect connectivity from deployment region only
- No HA — Fly machine restart = brief monitoring gap
- Restore-based DR via Litestream (not automatic failover)

**v2 migration path (designed for, not built):**
- Move persistence to Postgres (Ecto makes this a config change)
- Leader election for scheduling (only one node runs health checks)
- Region-scoped probe workers reporting to central store
- Data model already includes `region` field for future use

### Litestream

Continuous SQLite replication to S3.

- Provides point-in-time backup/restore, not HA failover
- Restore may lose most recent in-flight writes (replication lag)
- Recovery workflow documented and tested as part of deployment

## Data Model

### Errors (raw events)

```sql
CREATE TABLE errors (
  id TEXT PRIMARY KEY,              -- ERR-<nanoid>
  service TEXT NOT NULL,
  error_class TEXT NOT NULL,
  message TEXT NOT NULL,
  message_template TEXT,
  stack_trace TEXT,
  context TEXT,                     -- JSON, max 8KB enforced at ingest
  severity TEXT DEFAULT 'error',    -- error | warning | info
  environment TEXT DEFAULT 'production',
  group_hash TEXT NOT NULL,
  fingerprint TEXT,                 -- optional client-provided override for grouping
  region TEXT,                      -- probe region (future: multi-region)
  created_at TEXT NOT NULL
);

CREATE INDEX idx_errors_service_created ON errors(service, created_at DESC);
CREATE INDEX idx_errors_group_hash ON errors(group_hash, created_at DESC);
```

### Error Groups (rollup table)

Maintained on every error insert via upsert. Avoids expensive aggregation on raw events for every query.

```sql
CREATE TABLE error_groups (
  group_hash TEXT PRIMARY KEY,
  service TEXT NOT NULL,
  error_class TEXT NOT NULL,
  message_template TEXT,
  severity TEXT NOT NULL,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  total_count INTEGER NOT NULL DEFAULT 1,
  last_error_id TEXT NOT NULL,      -- FK to most recent raw error
  status TEXT DEFAULT 'active'      -- active | resolved | muted
);

CREATE INDEX idx_error_groups_service ON error_groups(service, last_seen_at DESC);
```

On ingest:
1. INSERT into `errors`
2. UPSERT into `error_groups` (increment count, update last_seen_at, last_error_id)
3. If INSERT (not UPDATE) → `error.new_class` webhook
4. If UPDATE and `last_seen_at` was >24h ago → `error.regression` webhook

### Health Check Targets (config)

```sql
CREATE TABLE targets (
  id TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  name TEXT NOT NULL,               -- human label: "cadence-api"
  service TEXT,                     -- canonical service identity, defaults to name
  method TEXT DEFAULT 'GET',        -- GET | HEAD
  headers TEXT,                     -- JSON object of custom headers
  interval_ms INTEGER DEFAULT 60000,
  timeout_ms INTEGER DEFAULT 10000,
  expected_status TEXT DEFAULT '200', -- single code, range "200-299", or comma-separated "200,204"
  body_contains TEXT,               -- optional substring match on response body
  degraded_after INTEGER DEFAULT 1, -- failures before degraded
  down_after INTEGER DEFAULT 3,     -- consecutive failures before down
  up_after INTEGER DEFAULT 1,       -- successes before recovery to up
  active INTEGER DEFAULT 1,
  created_at TEXT NOT NULL
);
```

### Target Checks (probe history)

Short-retention history for debugging "what happened?"

```sql
CREATE TABLE target_checks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  target_id TEXT NOT NULL,
  checked_at TEXT NOT NULL,
  status_code INTEGER,
  latency_ms INTEGER,
  result TEXT NOT NULL,             -- success | timeout | dns_error | tls_error | status_mismatch | body_mismatch | connection_error
  tls_expires_at TEXT,              -- certificate expiry date if available
  error_detail TEXT,                -- human-readable error message on failure
  region TEXT                       -- future: multi-region
);

CREATE INDEX idx_target_checks_target ON target_checks(target_id, checked_at DESC);
```

**Retention:** 7 days or 10,000 rows per target, whichever is smaller.

### Service Events (timeline)

Canonical append-only observability facts. Timeline queries and outbound
webhooks share the same payload model.

```sql
CREATE TABLE service_events (
  id TEXT PRIMARY KEY,              -- EVT-<nanoid>
  service TEXT NOT NULL,
  event TEXT NOT NULL,
  entity_type TEXT NOT NULL,        -- error_group | target | incident
  entity_ref TEXT,
  severity TEXT,
  summary TEXT NOT NULL,
  payload TEXT NOT NULL,            -- JSON, same envelope used for webhooks
  created_at TEXT NOT NULL
);

CREATE INDEX idx_service_events_service_created ON service_events(service, created_at DESC, id DESC);
CREATE INDEX idx_service_events_created ON service_events(created_at DESC, id DESC);
```

### Target State (runtime, separate from config)

```sql
CREATE TABLE target_state (
  target_id TEXT PRIMARY KEY,
  state TEXT DEFAULT 'unknown',     -- unknown | up | degraded | down
  consecutive_failures INTEGER DEFAULT 0,
  consecutive_successes INTEGER DEFAULT 0,
  last_checked_at TEXT,
  last_success_at TEXT,
  last_failure_at TEXT,
  last_transition_at TEXT,          -- when state last changed
  sequence INTEGER DEFAULT 0       -- monotonic counter for webhook ordering
);
```

### Webhook Subscriptions

```sql
CREATE TABLE webhooks (
  id TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  events TEXT NOT NULL,             -- JSON array: ["health_check.down", "error.new_class", ...]
  secret TEXT NOT NULL,             -- HMAC signing secret
  active INTEGER DEFAULT 1,
  created_at TEXT NOT NULL
);
```

### API Keys

```sql
CREATE TABLE api_keys (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,               -- human label: "cadence-prod"
  key_prefix TEXT NOT NULL,         -- first 8 chars for identification: "sk_live_a"
  key_hash TEXT NOT NULL,           -- bcrypt or sha256 of full key
  created_at TEXT NOT NULL,
  revoked_at TEXT                   -- NULL if active
);
```

Keys shown once on creation, stored hashed. Compared in constant time.

## Health Checker

### State Machine

```
unknown ──(first success)──→ up
unknown ──(first failure)──→ degraded

up ──(degraded_after failures)──→ degraded
degraded ──(up_after successes)──→ up
degraded ──(down_after consecutive failures)──→ down
down ──(up_after successes)──→ up

any ──(manual pause)──→ paused
paused ──(manual resume)──→ unknown  (re-evaluates on next check)

degraded/down ──(>N transitions in M minutes)──→ flapping
flapping ──(stable for M minutes)──→ (actual state)
```

Thresholds configurable per target. Defaults: `degraded_after=1`, `down_after=3`, `up_after=1`.

**Additional states:**
- **paused** — intentionally suppressed via CLI. No probes run, no alerts fire. Used during maintenance windows.
- **flapping** — rapid state oscillation (>4 transitions in 10 minutes). Suppresses webhook notifications until stable. Prevents alert fatigue from intermittent network issues.

### Probe Behavior

- HTTP method: configurable (GET or HEAD), default GET
- Custom request headers (e.g., Authorization for authenticated health endpoints)
- Follow redirects: up to 3 hops, re-validate SSRF on each redirect destination
- Timeout: configurable per-target, default 10s (connect + read)
- Expected status: single code, range, or set (`200`, `200-299`, `200,204`)
- Optional body assertion: substring match (`body_contains`)
- TLS metadata: extract certificate expiry date on successful HTTPS probes
- Record: status_code, latency_ms, result category, tls_expires_at

### Scheduling

- Each target gets its own GenServer under DynamicSupervisor
- Interval-based scheduling using monotonic clock (not "interval after completion" — prevents drift)
- Jitter: ±10% random offset on interval, persistent per-target seed (survives restarts)
- Shared Finch connection pool for all probes (not per-GenServer HTTP clients)

### SSRF Protection

Health check targets accept arbitrary URLs. Without validation, this is an SSRF engine.

**Mandatory protections:**
- Resolve hostname before connecting; validate all returned IPs
- Block: loopback (127.0.0.0/8, ::1), private (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16), link-local (169.254.0.0/16, fe80::/10), metadata endpoints (169.254.169.254)
- Re-validate after redirects (redirect to blocked IP = failure)
- Restrict schemes to `http://` and `https://`
- Optional `allow_private_targets` config flag for internal monitoring (default: false)

### Webhook Events

Fired on state transitions:

| Event | When |
|-------|------|
| `health_check.degraded` | state transitions to degraded |
| `health_check.down` | state transitions to down |
| `health_check.recovered` | state transitions to up from degraded or down |
| `health_check.tls_expiring` | TLS cert expires in <14 days (daily, not per-check) |

Each event payload includes `sequence` (monotonic per-target) so consumers can ignore stale events.

## Error Ingest API

### `POST /api/v1/errors`

```json
{
  "service": "cadence",
  "error_class": "Ecto.NoResultsError",
  "message": "expected at least one result but got none for user 4a8f...",
  "stack_trace": "...",
  "context": {"user_id": "4a8f...", "endpoint": "/api/sessions"},
  "severity": "error",
  "environment": "production",
  "fingerprint": ["custom-group-key"]
}
```

**Required:** `service`, `error_class`, `message`
**Optional:** everything else (server defaults apply)

### Payload Limits

| Field | Max Size |
|-------|----------|
| Total body | 100KB |
| `message` | 4KB |
| `stack_trace` | 32KB |
| `context` JSON | 8KB, max depth 4 |
| `fingerprint` array | 5 elements, 256 chars each |

Requests exceeding limits get `413 Payload Too Large`.

### Grouping

Three strategies in priority order:

**1. Client fingerprint (highest priority):** If `fingerprint` array is provided, `group_hash = sha256(service || join(fingerprint, ":"))`. Clients control grouping for edge cases.

**2. Stack trace fingerprint (when stack_trace has ≥2 in-project frames):** `sha256(service || error_class || stack_fingerprint)` where `stack_fingerprint` is the top 5 in-project frames with line numbers stripped, joined by `|`. Format per frame: `module:function/arity` or `file:function`.

**3. Message template (fallback):** `sha256(service || error_class || message_template)`.

### Message Template Stripping

Applied to `message` field only, in this exact order (most specific patterns first to prevent partial matches). Stored as `message_template`.

```elixir
@normalization_rules [
  # 1. UUIDs — must match before hex strings
  {~r/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i, "<uuid>"},

  # 2. ISO 8601 timestamps
  {~r/\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?(?:Z|[+-]\d{2}:?\d{2})?\b/, "<timestamp>"},

  # 3. Email addresses — must match before @ gets consumed by other rules
  {~r/\b[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}\b/, "<email>"},

  # 4. File paths (Unix + Windows)
  {~r{(?:^|[\s('"=])(/[^\s'")\]]+)}, "<path>"},
  {~r/\b[A-Za-z]:\\(?:[^\\\s]+\\)*[^\\\s]+\b/, "<path>"},

  # 5. Long hex strings (>8 chars) — after UUIDs are already replaced
  {~r/\b(?:0x)?[0-9a-f]{9,}\b/i, "<hex>"},

  # 6. Integers with 4+ digits
  {~r/\b\d{4,}\b/, "<int>"},

  # 7. Collapse whitespace
  {~r/\s+/, " "}
]
```

**Example:**
```
Input:  "user 123456 failed at /app/lib/foo.ex:42 on 2026-03-14T18:00:00Z request 4a8f9c1d-1111-2222-3333-abcdefabcdef"
Output: "user <int> failed at <path> on <timestamp> request <uuid>"
```

### In-Project Frame Detection

For stack trace fingerprinting, a frame is "in-project" if:

1. Client sends structured frames with `"in_app": true` flag (preferred)
2. Frame module matches configured prefixes (e.g., `["Cadence", "Heartbeat"]`)
3. Frame file path contains configured roots (e.g., `["/app/lib/", "/app/apps/"]`)

```elixir
config :canary,
  in_project_module_prefixes: ["Cadence", "Heartbeat", "Overmind"],
  in_project_path_prefixes: ["/app/lib/", "/app/apps/"]
```

**Fallback chain:** If fewer than 2 in-project frames → use top 5 frames regardless of origin → if stack trace unparseable → fall back to message_template.

### Grouping Limitations (v1)

- Over-grouping: different root causes with same error class + similar template merge
- Under-grouping: format variations in messages may create duplicate groups
- No manual merge/split of groups

**Deduplication:** In-memory ETS cache of recent group_hashes with timestamps. If the same group_hash was seen within the last 60 seconds, still persist the error but suppress duplicate `error.new_class` webhooks.

### Rate Limiting

Per-API-key token bucket rate limiter (ETS-backed):
- **Default:** 100 errors/minute per API key
- **Burst:** up to 200 in a 10-second window
- **Response:** `429 Too Many Requests` with `Retry-After` header
- Configurable per-key override

### Response: `201 Created`

```json
{
  "id": "ERR-a1b2c3",
  "group_hash": "sha256...",
  "is_new_class": true
}
```

### Webhook Events

| Event | When |
|-------|------|
| `error.new_class` | First occurrence of a group_hash |
| `error.regression` | Group_hash recurs after 24h of silence |

**Cooldown:** After firing `error.new_class` or `error.regression` for a group_hash, suppress further webhooks for that group for 5 minutes. Prevents flood from exception loops.

## Query API

All responses include `summary` (natural language) and are bounded.

**Rate limiting:** 30 queries/minute per API key.

### `GET /api/v1/query?service=cadence&window=1h`

Recent errors for a service, pre-aggregated from `error_groups` table.

```json
{
  "summary": "3 errors in cadence in the last hour. 2 unique classes. Most frequent: Ecto.NoResultsError (2 occurrences).",
  "service": "cadence",
  "window": "1h",
  "total_errors": 3,
  "groups": [
    {
      "group_hash": "abc123",
      "error_class": "Ecto.NoResultsError",
      "count": 2,
      "first_seen": "2026-03-14T17:05:00Z",
      "last_seen": "2026-03-14T17:42:00Z",
      "sample_message": "expected at least one result...",
      "severity": "error",
      "status": "active"
    }
  ],
  "cursor": "eyJvZmZzZXQiOjUwfQ=="
}
```

**Pagination:** cursor-based, max 50 groups per page.
**Allowed windows:** `1h`, `6h`, `24h`, `7d`, `30d`

### `GET /api/v1/query?group_by=error_class&window=24h`

Cross-service error frequency analysis. Returns top 50 error classes by count.

### `GET /api/v1/errors/:id`

Full detail for a single error: stack trace, context, group metadata, occurrence count in group.

### `GET /api/v1/health-status`

All targets with current state, last check time, recent check history.

```json
{
  "summary": "5 targets monitored. 4 up, 1 degraded (cadence-api: 2 consecutive failures).",
  "targets": [
    {
      "name": "cadence-api",
      "url": "https://cadence.fly.dev/api/health",
      "state": "degraded",
      "consecutive_failures": 2,
      "last_checked_at": "2026-03-14T18:00:00Z",
      "last_success_at": "2026-03-14T17:55:00Z",
      "latency_ms": 245,
      "tls_expires_at": "2026-06-14T00:00:00Z",
      "recent_checks": [
        {"checked_at": "2026-03-14T18:00:00Z", "result": "status_mismatch", "status_code": 503, "latency_ms": 245},
        {"checked_at": "2026-03-14T17:59:00Z", "result": "timeout", "latency_ms": 10000}
      ]
    }
  ]
}
```

### `GET /api/v1/targets/:id/checks?window=24h`

Probe history for a specific target. Returns individual check results for debugging.

## Webhook Contract

### Semantics

**At-least-once, unordered, lossy on restart.**

Webhooks may be delivered more than once (use `X-Delivery-Id` to deduplicate). Order is not guaranteed across events, but `sequence` numbers within a target/group allow consumers to ignore stale state. In-flight deliveries are lost on process restart (no persistent queue in v1).

### Signing

All webhooks signed with HMAC-SHA256 using the subscription's secret.

### Headers

```
X-Signature: sha256=<hex digest of body>
X-Event: health_check.down
X-Delivery-Id: <uuid>
X-Webhook-Version: 1
X-Sequence: 42
Content-Type: application/json
```

### Payload: health_check events

```json
{
  "event": "health_check.down",
  "target": {
    "name": "cadence-api",
    "url": "https://cadence.fly.dev/api/health"
  },
  "state": "down",
  "previous_state": "degraded",
  "consecutive_failures": 3,
  "last_success_at": "2026-03-14T17:55:00Z",
  "last_check": {
    "status_code": 503,
    "latency_ms": 245,
    "result": "status_mismatch"
  },
  "sequence": 42,
  "timestamp": "2026-03-14T18:00:00Z"
}
```

### Payload: error events

```json
{
  "event": "error.new_class",
  "error": {
    "id": "ERR-a1b2c3",
    "service": "cadence",
    "error_class": "Ecto.NoResultsError",
    "message": "expected at least one result...",
    "severity": "error",
    "group_hash": "abc123"
  },
  "timestamp": "2026-03-14T18:05:00Z"
}
```

### Delivery

- **Retry:** 3 attempts with exponential backoff (1s, 5s, 30s)
- **Timeout:** 10s per delivery attempt
- **Idempotency:** `X-Delivery-Id` header for consumer deduplication
- **Circuit breaker:** After 10 consecutive delivery failures to a webhook URL, mark subscription as `suspended`. Probe periodically (every 5 min) to re-enable.
- **No dead letter queue** (v1) — failed deliveries are logged (structured), not persisted
- **Cooldown:** Per-group cooldown prevents webhook flood from flapping or exception loops (5 min default)
- **Webhook URL validation:** HTTPS required by default. HTTP allowed only with explicit `--allow-insecure` flag.

## Auth

- **API keys** — bearer token in `Authorization: Bearer sk_live_...` header
- **Format:** `sk_<environment>_<nanoid>` (e.g., `sk_live_a1b2c3d4e5f6`)
- **Storage:** key shown once on creation; stored as `(key_prefix, bcrypt(key))`
- **Comparison:** constant-time
- **Scoping:** keys are global (v1). Internal tool, not multi-tenant.
- **Management:** CLI commands to generate, list, revoke
- **Webhook signing:** separate per-subscription HMAC secrets, not API keys
- **Auth failure throttling:** per-IP rate limit on failed auth attempts (10/min)

## Client Strategy

### Wire Protocol (language-agnostic)

The HTTP API is the primary interface. Any language that can make HTTP requests can use Canary.

Publish:
- **OpenAPI 3.1 spec** — machine-readable API definition
- **JSON Schema** — for error ingest payload validation
- **curl examples** — in README and docs

### Reference Clients

| Language | Scope | Priority |
|----------|-------|----------|
| **Elixir** | Full client + Logger backend | Phase 2 (our services) |
| **Python** | Thin HTTP wrapper, ~50 LOC | Phase 3 (community) |
| **TypeScript** | Thin HTTP wrapper, ~50 LOC | Phase 3 (community) |
| **Go** | Thin HTTP wrapper, ~50 LOC | Phase 3 (community) |

The Elixir client is the reference implementation. Other language clients are thin wrappers around `POST /api/v1/errors` — not full SDKs.

### Elixir Client

```elixir
# mix.exs
{:canary_client, "~> 0.1"}

# config
config :canary_client,
  endpoint: "https://canary.fly.dev",
  api_key: System.get_env("CANARY_API_KEY"),
  service: "cadence"

# explicit capture
Canary.capture(%RuntimeError{message: "boom"}, context: %{user_id: id})
Canary.capture(error, stacktrace, context: %{endpoint: "/api/sessions"})

# with fingerprint override
Canary.capture(error, fingerprint: ["payment-flow", "stripe-webhook"])
```

Logger backend deferred to v2 (risk of noise; explicit capture gives better stack traces and context).

## Self-Observability

Who watches the watcher?

### Internal Endpoints

| Endpoint | Purpose |
|----------|---------|
| `GET /healthz` | Process liveness — returns 200 if HTTP router is alive |
| `GET /readyz` | Full readiness — checks SQLite connectivity, Litestream status, supervisor health |

### Structured Logging

JSON-formatted logs for all operations:
- Error ingest: service, group_hash, is_new_class, rate_limited
- Health checks: target, result, latency_ms, state_transition
- Webhook delivery: subscription_id, event, attempt, success/failure, response_code
- Auth: key_prefix, success/failure, IP

### Fly.io Health Check

Configure Fly.io to probe `/healthz` — auto-restart if unresponsive.

## CLI

**What it is:** A thin HTTP client wrapping the admin API. Contains zero business logic — only argument parsing, HTTP calls, and response formatting. Configured via `CANARY_ENDPOINT` and `CANARY_API_KEY` environment variables.

**Distribution (v1):** Escript (requires Erlang runtime). Every command maps 1:1 to an HTTP endpoint. README documents curl equivalents first, CLI second.

**Distribution (v2, if demand):** Replace with Go binary for cross-platform static distribution. Same interface, same API calls underneath. The CLI is a shallow module by design — replacing its implementation changes nothing about the system.

**Rule:** The API is the source of truth. The CLI is syntactic sugar. Any operation possible via CLI must be equally possible via curl.

```bash
# Health check management
canary targets list
canary targets add cadence-api https://cadence.fly.dev/api/health \
  --interval 60s --method GET --expected-status 200-299
canary targets add internal-api https://internal.local/health \
  --allow-private  # explicit opt-in for private URLs
canary targets pause cadence-api
canary targets resume cadence-api
canary targets remove cadence-api

# Error queries
canary errors recent --service cadence --window 1h
canary errors show ERR-a1b2c3
canary errors frequency --window 24h
canary errors mute <group-hash>        # suppress webhooks for this group

# Health status
canary status
canary checks cadence-api --window 24h  # probe history

# Webhook management
canary webhooks add https://example.com/hook \
  --events health_check.down,error.new_class
canary webhooks list
canary webhooks test <id>               # send test payload
canary webhooks remove <id>

# API key management
canary keys generate --name "cadence-prod"
canary keys list
canary keys revoke <key-id>
```

## Configuration

**DB is canonical runtime config.** Config file seeds initial state on first boot.

```elixir
# config/runtime.exs — environment-driven
config :canary,
  port: System.get_env("PORT", "4000") |> String.to_integer(),
  api_key_salt: System.fetch_env!("API_KEY_SALT"),
  litestream_s3_bucket: System.get_env("LITESTREAM_S3_BUCKET"),
  allow_private_targets: System.get_env("ALLOW_PRIVATE_TARGETS", "false") == "true",
  error_retention_days: System.get_env("ERROR_RETENTION_DAYS", "30") |> String.to_integer(),
  check_retention_days: System.get_env("CHECK_RETENTION_DAYS", "7") |> String.to_integer()

# config/seeds.exs — initial targets, loaded on first boot only
[
  %{name: "cadence-api", url: "https://cadence.fly.dev/api/health", interval_ms: 60_000},
  %{name: "heartbeat", url: "https://heartbeat.fly.dev/api/health", interval_ms: 60_000},
  %{name: "overmind", url: "https://overmind.fly.dev/api/health", interval_ms: 120_000}
]
```

## Graceful Shutdown

On SIGTERM (Fly.io deploy/restart):

1. Stop accepting new HTTP requests (Bandit `shutdown_timeout: 15_000`)
2. Cancel next scheduled health checks (don't start new probes)
3. Wait for in-flight probes to complete (up to 10s)
4. Finish in-flight webhook deliveries (up to 10s) — abandon retries
5. Flush SQLite WAL checkpoint
6. Persist checker state to `target_state` table
7. Exit

Total shutdown budget: 30 seconds (Fly.io default `kill_timeout`).

## Retention

| Data | Default Retention | Pruning |
|------|-------------------|---------|
| `errors` | 30 days | Daily scheduled task |
| `error_groups` | Indefinite (summary data) | N/A |
| `target_checks` | 7 days or 10,000 per target | Daily scheduled task |
| `targets`, `target_state` | Indefinite | Manual via CLI |
| `webhooks` | Indefinite | Manual via CLI |
| `api_keys` | Indefinite (revoked keys kept for audit) | N/A |

Pruning runs as a scheduled Elixir task, paginated (1000 rows per batch, loop until empty).

## Architecture

```
lib/
  canary/
    application.ex           # Supervision tree root
    health/
      checker.ex             # GenServer per target — periodic HTTP probes
      state_machine.ex       # State transitions with configurable thresholds + flap detection
      supervisor.ex          # DynamicSupervisor for checker GenServers
      ssrf_guard.ex          # URL validation — block private/loopback IPs
      probe.ex               # HTTP probe execution via shared Finch pool
    errors/
      ingest.ex              # POST /api/v1/errors — validate, group, persist
      grouping.ex            # message_template extraction, stack fingerprint, sha256
      rate_limiter.ex        # ETS-backed token bucket per API key
      dedup_cache.ex         # ETS cache for webhook deduplication window
    store/
      repo.ex                # Ecto.Repo (pool_size: 1 for writes)
      read_repo.ex           # Ecto.Repo (pool_size: 4, read-only)
      migrations.ex          # Schema migrations run on boot
    query.ex                 # GET /api/v1/query — reads from error_groups, pagination
    workers/
      webhook_delivery.ex    # Oban worker — webhook dispatch + retries
      retention_prune.ex     # Oban worker — scheduled pruning
    alerter/
      signer.ex              # HMAC-SHA256 webhook signing
      circuit_breaker.ex     # Per-subscription health tracking (ETS)
      cooldown.ex            # Per-group webhook suppression (ETS)
    auth.ex                  # API key validation, constant-time comparison
  canary_web/
    router.ex                # Phoenix router, payload size limits
    controllers/
      error_controller.ex    # POST /api/v1/errors
      query_controller.ex    # GET /api/v1/query, GET /api/v1/errors/:id
      health_controller.ex   # GET /api/v1/health-status, /healthz, /readyz
      target_controller.ex   # Target CRUD (for CLI)
      webhook_controller.ex  # Webhook subscription CRUD (for CLI)
      key_controller.ex      # API key management (for CLI)
    plugs/
      auth.ex                # API key auth plug
      rate_limit.ex          # Rate limiting plug
  canary_client/             # Separate hex package
    client.ex                # HTTP client for error reporting
config/
  config.exs
  runtime.exs                # Fly.io secrets, feature flags
  seeds.exs                  # Initial targets (first boot only)
```

### Supervision Tree

```
Canary.Application
├── Canary.Repo                  # Ecto write repo (pool_size: 1)
├── Canary.ReadRepo              # Ecto read repo (pool_size: 4)
├── Oban                         # Job processing (Lite engine, SQLite)
│   ├── Webhook delivery workers
│   └── Retention pruning workers
├── Canary.Health.Supervisor     # DynamicSupervisor for checkers
│   ├── Canary.Health.Checker    # one per target
│   ├── Canary.Health.Checker
│   └── ...
├── Canary.Errors.RateLimiter    # ETS token buckets
├── Canary.Errors.DedupCache     # ETS dedup window
├── Canary.Alerter.CircuitBreaker  # ETS per-subscription health
├── Finch (named: Canary.Finch)  # Shared HTTP connection pool
└── CanaryWeb.Endpoint           # Phoenix + Bandit HTTP server
```

## Phased Delivery

### Phase 1: Health Checker

Replaces Uptime Robot. Smallest useful surface.

- [ ] Supervision tree with DynamicSupervisor for health checkers
- [ ] GenServer per target with configurable interval + jitter (monotonic clock)
- [ ] State machine with configurable thresholds (degraded_after, down_after, up_after)
- [ ] Shared Finch connection pool for probes
- [ ] SSRF guard — block private/loopback/metadata IPs
- [ ] HEAD/GET support, custom headers, expected status ranges, optional body assertion
- [ ] TLS certificate expiry metadata extraction
- [ ] Probe history persistence (`target_checks` table)
- [ ] Probe failure reason taxonomy (timeout, dns_error, tls_error, status_mismatch, body_mismatch, connection_error)
- [ ] `GET /api/v1/health-status` with summary field
- [ ] `GET /api/v1/targets/:id/checks` probe history endpoint
- [ ] Webhook dispatch on state transitions (HMAC-signed, retries, circuit breaker)
- [ ] Webhook subscription management via CLI
- [ ] Target management via CLI (add, remove, pause, resume, list)
- [ ] API key auth (hashed storage, constant-time comparison)
- [ ] Rate limiting on auth failures
- [ ] SQLite persistence with WAL + PRAGMAs + write serialization
- [ ] Litestream backup to S3
- [ ] Graceful shutdown (drain probes, flush WAL)
- [ ] Self-observability: `/healthz`, `/readyz`, structured JSON logging
- [ ] OpenAPI spec for health endpoints
- [ ] Deployed to Fly.io, monitoring heartbeat + cadence + overmind
- [ ] **Gate:** 1 week parallel operation with Uptime Robot before decommission

### Phase 2: Error Ingestion

Replaces Sentry capture.

- [ ] `POST /api/v1/errors` — validate schema, enforce payload limits, extract message_template, compute group_hash, persist
- [ ] `error_groups` rollup table with upsert on ingest
- [ ] Client-provided fingerprint override for grouping
- [ ] Per-API-key rate limiting (ETS token bucket)
- [ ] In-memory dedup cache for webhook suppression
- [ ] Webhook on `error.new_class` and `error.regression` with cooldown
- [ ] Webhook sequence numbers for ordering
- [ ] Client library (Elixir) with explicit `Canary.capture/2`
- [ ] `canary errors recent` and `canary errors show` CLI commands
- [ ] `canary errors mute` for webhook suppression
- [ ] Retention pruning (30-day default, paginated)
- [ ] OpenAPI spec for error endpoints + JSON Schema for ingest payload
- [ ] **Gate:** One service migrated off Sentry SDK as proof

### Phase 3: Query API + Client Ecosystem

New capability — the agent-first differentiator.

- [ ] `GET /api/v1/query` — service filter, time window, group_by, cursor pagination
- [ ] `GET /api/v1/errors/:id` — full detail with stack trace and context
- [ ] Natural language `summary` field in all query responses
- [ ] Response size bounded (max 50 groups, cursor pagination)
- [ ] Query rate limiting (30/min per key)
- [ ] Claude Code skill file for querying from agent sessions
- [ ] Reference clients: Python, TypeScript, Go (thin HTTP wrappers)
- [ ] curl examples in docs
- [ ] **Gate:** agent can query and act on error data in a real debugging session

### Phase 4: Migration & Decommission

- [ ] All services migrated to client library
- [ ] Sentry SDKs removed from all repos
- [ ] Uptime Robot account closed
- [ ] Sentry account downgraded/closed
- [ ] Logger backend (v2, optional)

## Non-Goals (v1)

- Multi-tenant / multi-org support
- Distributed tracing / spans
- Log aggregation (structured errors only)
- Metrics / time-series data
- Browser SDK / client-side error capture
- MCP server
- Built-in GitHub/Slack/Discord integrations
- Multi-region health checking
- High availability / automatic failover
- Push-based monitoring / heartbeats (cron job monitoring)
- Status pages

## Future Considerations (v2+)

Designed for in data model, not built in v1:

- **Multi-region probing** — `region` field in target_checks, probe workers in multiple Fly regions, consensus-based down detection
- **Push-based monitoring** — `POST /api/v1/ping/:target_id` heartbeat endpoint for cron jobs and batch processes. Mark target as down if no ping within interval.
- **Postgres migration** — Ecto abstraction layer makes this a config change, not a rewrite
- **Logger backend** — automatic capture from Elixir Logger, with noise filtering
- **Additional check types** — TCP port, DNS resolution, TLS-only (cert validation without full request)

## Decisions Made

| Question | Decision | Rationale |
|----------|----------|-----------|
| Language | Elixir/OTP | Best fit for stateful periodic actors + fault isolation. Our team ships Elixir. Accept narrower contributor pool. |
| Database | SQLite (Ecto.SQLite3) + Litestream | Single-file, WAL handles our volume, Ecto preserves Postgres migration path. |
| HTTP framework | Phoenix (`--no-html`) + Bandit | Router, plug pipeline, telemetry for free. Zero HTML overhead. Community consensus over pure Plug. |
| HTTP client | Req + Finch | Connection pooling, redirect following, timeout control, decompression. |
| Background jobs | Oban (Lite engine) | Webhook delivery + retries, retention pruning, TLS scans. Stateless retry-oriented work. |
| Error grouping | sha256(service, error_class, message_template) + optional fingerprint | Simple, deterministic. Client override for edge cases. |
| Retention | 30d errors, 7d checks | Bounded storage without losing debugging context. |
| Auth | Global API keys, hashed storage | Internal tool. Multi-tenant is a non-goal. |
| Consumer integrations | None (generic webhooks) | Consumers define behavior. Don't bake in assumptions. |
| MCP server | No | API + CLI + skill files. |
| Dashboard | None | Agents are the UI. |
| Config source | DB canonical, file seeds on boot | Long-running service needs dynamic config. |
| API versioning | `/api/v1/...` | Forward compatibility. |
| Webhook semantics | At-least-once, unordered, sequence numbers | Simple, honest, composable. |
| Write concurrency | Ecto repo pool_size: 1 | SQLite is single-writer; serialize at app layer. |
| HA | None (v1) | Documented constraint. Restore-based DR via Litestream. |
| Error responses | RFC 9457 Problem Details | Standard, machine-readable, agent-friendly. ~30 LOC Plug module. |
| Summary generation | Deterministic template strings | Fast, reliable, zero dependencies. No LLM on request path. |
| CLI | Thin HTTP client (escript v1, Go v2 if demand) | API is canonical. CLI is syntactic sugar with zero business logic. |
| GenServer vs Oban | GenServers own probes/state, Oban owns webhook delivery + maintenance | Stateful cadence-sensitive work → GenServer. Stateless retry-oriented work → Oban. |
| First boot detection | `seed_runs` table marker | Transactional, survives container replacement, auditable. |

## Error Response Format

All error responses use [RFC 9457 Problem Details](https://www.rfc-editor.org/rfc/rfc9457) (obsoletes RFC 7807) with `Content-Type: application/problem+json`. No Elixir library exists — implement as a ~30 LOC Plug module.

```json
{
  "type": "https://canary.dev/problems/rate-limited",
  "title": "Rate Limit Exceeded",
  "status": 429,
  "detail": "API key exceeded 100 errors per minute.",
  "code": "rate_limited",
  "retry_after": 42,
  "request_id": "req_a1b2c3"
}
```

**Standard error codes:**

| Status | Code | When |
|--------|------|------|
| 400 | `invalid_request` | Malformed JSON, missing required fields |
| 401 | `invalid_api_key` | Missing, malformed, or revoked API key |
| 404 | `not_found` | Resource doesn't exist |
| 413 | `payload_too_large` | Body exceeds size limits |
| 422 | `validation_error` | Semantically invalid field values (includes `errors` map) |
| 429 | `rate_limited` | Token bucket exhausted (includes `retry_after`) |
| 500 | `internal_error` | Unexpected server error |
| 503 | `unavailable` | DB not ready, service starting up |

**Validation errors** include field-level detail:

```json
{
  "type": "https://canary.dev/problems/validation-error",
  "title": "Validation Error",
  "status": 422,
  "detail": "Request body has invalid fields.",
  "code": "validation_error",
  "errors": {
    "service": ["can't be blank"],
    "severity": ["must be one of: error, warning, info"]
  },
  "request_id": "req_d4e5f6"
}
```

All error responses include `request_id` for correlation with structured logs.

## Summary Generation

All query responses include a `summary` field. Generated from **deterministic template strings** — pure, synchronous, local. No LLM calls, no external dependencies, no probabilistic behavior.

**Templates:**

```elixir
# Error query summary
"#{total} errors in #{service} in the last #{window}. " <>
"#{unique_count} unique classes. " <>
"Most frequent: #{top_class} (#{top_count} occurrences)."

# Health status summary
"#{total} targets monitored. #{up_count} up" <>
if(degraded_count > 0, do: ", #{degraded_count} degraded") <>
if(down_count > 0, do: ", #{down_count} down") <>
"."

# Single error detail summary
"#{error_class} in #{service}. " <>
"Seen #{count} times since #{first_seen}. " <>
"Last occurrence: #{last_seen}."
```

**Design rule:** Summary generation is a pure function `(query_result) -> string`. No side effects. Tested with unit tests.

**Upgrade path (v2+):** If richer summaries are ever needed, add optional `summary_ai` field generated asynchronously — never on the request path.

## GenServer / Oban Separation

**Principle:** Stateful cadence-sensitive work → GenServer. Stateless retry-oriented work → Oban.

### GenServers own:

- Per-target schedule (monotonic clock + jitter)
- In-memory state (consecutive_failures, current_state, flap detection)
- Probe execution via `Canary.Health.Probe` (direct HTTP call through shared Finch pool)
- State machine transition decisions
- Persistence of check results and state transitions to DB
- Enqueueing Oban jobs on state transitions

### Oban owns:

- **Webhook delivery** — `Canary.Workers.WebhookDelivery` with `max_attempts: 4`, exponential backoff
- **Retention pruning** — `Canary.Workers.RetentionPrune` scheduled daily via Oban cron
- **TLS expiry scan** — `Canary.Workers.TlsScan` scheduled daily, fires `health_check.tls_expiring` webhooks

### Probe lifecycle (concrete):

```
1. GenServer timer fires (Process.send_after)
2. GenServer calls Canary.Health.Probe.check(target, finch_pool)
3. Probe returns {:ok, result} or {:error, reason}
4. GenServer writes target_checks row via Repo
5. GenServer computes new state via StateMachine.transition(current, result)
6. If state changed:
   a. Update target_state via Repo
   b. Oban.insert(WebhookDelivery.new(%{event: ..., payload: ...}))
7. GenServer schedules next check (Process.send_after with jitter)
```

### Target lifecycle management:

`Canary.Health.Manager` (GenServer) loads active targets from DB on boot, starts one checker per target via DynamicSupervisor. Watches for target CRUD operations and starts/stops checkers dynamically.

## Testing Strategy

### Layers

| Layer | Scope | Tools | Async? |
|-------|-------|-------|--------|
| **Unit** | Pure functions: StateMachine, Grouping, TemplateStripping, Summary | ExUnit | Yes |
| **Integration** | Repo + Endpoint + Oban + HTTP stubs | Bypass, Oban.Testing, Ecto sandbox | No |
| **System** | Full app boot with temp DB | Bypass for probes/webhooks | No |

### Health checker

- **Req.Test** (preferred, since we already use Req) or **Bypass** simulates target responses: 200, 503, timeout, redirect, bad body, SSRF redirect. `Req.Test.transport_error(conn, reason)` simulates network failures.
- Test state machine transitions as pure functions: `StateMachine.transition(state, event) -> {new_state, side_effects}`
- Table-driven tests for all transition paths including flap detection
- Inject `:check_now` message in tests to avoid wall-clock sleeps
- Assert DB writes (target_checks, target_state) after probe

### Webhook delivery

- **Oban.Testing** with `perform_job/3` for unit-level worker tests
- **Bypass** captures outbound HTTP for integration tests
- Assert: HMAC signature correctness, header presence, retry behavior, circuit breaker activation

### SQLite concurrency

- Real SQLite file (not in-memory) for concurrency tests
- Spawn concurrent tasks inserting errors + health check updates
- Assert no lost writes, correct busy_timeout behavior
- Run with `async: false`

### Error ingest pipeline

- End-to-end: POST → grouping → persistence → webhook enqueueing
- Property-based tests for template stripping (random strings with embedded UUIDs/timestamps should normalize consistently)

## Development Environment

**One-command setup:** `mix setup && mix phx.server`

No Docker required for local development. The app has zero external dependencies.

### Setup

```bash
git clone https://github.com/misty-step/canary && cd canary
cp .env.example .env
mix setup          # deps.get + ecto.create + ecto.migrate
mix phx.server     # starts on localhost:4000
```

### Dev helpers (Mix tasks)

```bash
mix canary.seed                    # Load config/seeds.exs (targets, webhook, API key)
mix canary.dev.generate_errors 100 # Insert N fake errors across services
mix canary.dev.check_now cadence   # Force immediate probe of a target
mix canary.dev.webhook_sink        # Start local webhook receiver on :4001, logs payloads
```

### .env.example

```bash
PORT=4000
SECRET_KEY_BASE=generate-with-mix-phx-gen-secret
CANARY_DB_PATH=./canary_dev.db
# Optional: Litestream (not needed for local dev)
# LITESTREAM_REPLICA_URL=s3://bucket/canary.db
```

### Webhook testing

The `webhook_sink` mix task starts a tiny Bandit server that logs received payloads with signature verification. Developers can also use `webhook.site` or `ngrok` for external testing.

## Deployment

### Dockerfile

Multi-stage build. Litestream runs as entrypoint supervisor.

```dockerfile
# Build stage
FROM hexpm/elixir:1.17-erlang-27-debian-bookworm AS build
WORKDIR /app
COPY mix.exs mix.lock ./
RUN mix deps.get --only prod && mix deps.compile
COPY config config
COPY lib lib
COPY priv priv
ENV MIX_ENV=prod
RUN mix release

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libstdc++6 openssl libncurses5 locales ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=litestream/litestream:latest /usr/local/bin/litestream /usr/local/bin/litestream
COPY --from=build /app/_build/prod/rel/canary /app
COPY litestream.yml /etc/litestream.yml
COPY bin/entrypoint.sh /app/bin/entrypoint.sh
WORKDIR /app
CMD ["/app/bin/entrypoint.sh"]
```

### bin/entrypoint.sh

```bash
#!/bin/bash
set -e

DB_PATH="${CANARY_DB_PATH:-/data/canary.db}"

# Restore from Litestream if DB doesn't exist locally
if [ ! -f "$DB_PATH" ] && [ -n "$LITESTREAM_REPLICA_URL" ]; then
  litestream restore -if-replica-exists -o "$DB_PATH" "$LITESTREAM_REPLICA_URL"
fi

# Start app under Litestream (continuous replication)
if [ -n "$LITESTREAM_REPLICA_URL" ]; then
  exec litestream replicate -exec "/app/bin/canary start" "$DB_PATH" "$LITESTREAM_REPLICA_URL"
else
  exec /app/bin/canary start
fi
```

### fly.toml

```toml
app = "canary"
primary_region = "iad"
kill_signal = "SIGTERM"
kill_timeout = "30s"

[build]
  dockerfile = "Dockerfile"

[mounts]
  source = "canary_data"
  destination = "/data"

[env]
  CANARY_DB_PATH = "/data/canary.db"
  PHX_HOST = "canary.fly.dev"
  PORT = "4000"

[http_service]
  internal_port = 4000
  force_https = true

  [[http_service.checks]]
    grace_period = "10s"
    interval = "30s"
    method = "GET"
    path = "/healthz"
    timeout = "5s"
```

### litestream.yml

```yaml
dbs:
  - path: /data/canary.db
    replicas:
      - type: s3
        bucket: ${LITESTREAM_S3_BUCKET}
        path: canary.db
        region: ${LITESTREAM_S3_REGION}
        access-key-id: ${LITESTREAM_ACCESS_KEY_ID}
        secret-access-key: ${LITESTREAM_SECRET_ACCESS_KEY}
        snapshot-interval: 1h
```

## First Boot & Seeds

**Detection:** `seed_runs` table tracks which seeds have been applied.

```sql
CREATE TABLE seed_runs (
  seed_name TEXT PRIMARY KEY,
  applied_at TEXT NOT NULL
);
```

**On application start:**
1. Run Ecto migrations
2. Check if `seed_runs` contains `initial_config_v1`
3. If not: parse `config/seeds.exs`, insert targets/webhooks/default API key idempotently, insert marker row
4. If yes: skip

**Idempotency:** Seeds use `INSERT ... ON CONFLICT (name) DO NOTHING` for targets and webhooks. Never overwrite user changes after first boot.

**Seeds are bootstrap only, not ongoing reconciliation.** After first boot, all config management happens through the API/CLI.
