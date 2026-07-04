# Canary

Open-source, self-hosted observability for agent-driven infrastructure.

Ingests errors, probes health, correlates incidents, keeps timelines, and answers queries. Built for AI agents and operators.

## Why

Existing tools (Sentry, Uptime Robot) are designed around humans staring at dashboards or bespoke downstream integrations. Canary is designed for AI agents and operators who need structured, bounded, queryable observability data.

- **One service** replaces Sentry error capture + Uptime Robot health monitoring
- **Agent-first responses** with natural-language summaries and bounded payloads
- **Timelines and incidents** — deterministic correlation without an LLM in the loop
- **Generic webhooks** — consumers define their own behavior
- **Self-hosted** on Fly.io with SQLite + Litestream + Fly Tigris backup

Read the product direction in [`VISION.md`](VISION.md). The agent UX laws and
coordination philosophy live in
[`docs/agent-first-identity.md`](docs/agent-first-identity.md).

## Quick Start

```bash
git clone https://github.com/misty-step/canary && cd canary
cp .env.example .env
set -a; . ./.env; set +a
./bin/bootstrap    # installs deps for the core service and SDK packages
cargo run -p canary-server
```

No Docker is required for local development outside the Dagger gate. The repo
includes the Rust service workspace and the TypeScript SDK package.

### First run: capturing the bootstrap API key

On first boot, Canary seeds a one-time bootstrap admin key and prints it to
**stderr**. Capture it immediately — it is not shown again:

```bash
set -a; . ./.env; set +a
cargo run -p canary-server 2>&1 | grep "Bootstrap API key:"
```

Store the key as an environment variable for API calls:

```bash
export CANARY_ADMIN_KEY="sk_live_..."
```

All authenticated endpoints require a scoped API key. The bootstrap key has
`admin` scope. Create scoped keys for services and operators:

```bash
curl -fsS -X POST http://localhost:4000/api/v1/keys \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "my-app-ingest", "scope": "ingest-only"}'
```

See [`docs/api-key-rotation.md`](docs/api-key-rotation.md) for the full scope
matrix and rotation procedure.
Responder incident context uses a redacted envelope, service-bound
`responder-write` read authority, and durable read-audit events; see
[`docs/responder-context-safety.md`](docs/responder-context-safety.md).

For Fly deployment (including key recovery if the first boot log was missed),
see [`docs/self-host-fly.md`](docs/self-host-fly.md).

Canary has no human dashboard by design — agents are the UI. Operators who
need to look at current state use the query API directly (`GET
/api/v1/status`, `GET /api/v1/report`, `GET /api/v1/query`, `GET
/api/v1/errors/{id}`) and use the DR scripts in `bin/` for storage and
backup checks. See
[`docs/operator-dashboard-removal.md`](docs/operator-dashboard-removal.md)
for the decision record.

## Development

This is a monorepo with two maintained package surfaces:

- `crates/` — Canary Rust service workspace
- `clients/typescript/` — TypeScript SDK

### Toolchain

Supported local toolchains are pinned in `.tool-versions`:

- Rust `1.94.0`
- Node.js `22.22.0`

Local validation also requires the `dagger` CLI, pinned to the version declared
in `dagger.json`. On macOS, repo-local Dagger uses the active local Docker
client first and falls back to Colima-over-SSH if direct Docker access is
unavailable. GitHub Actions and git hooks delegate to the same pinned Dagger
surface.

The production Dockerfile builds the Rust `canary-server` binary, and CI uses
the same pinned toolchain versions.

### Production Evidence

Rust production claims are evidence-scoped:

- [docs/architecture/rust-cutover-evidence-2026-06-06.md](docs/architecture/rust-cutover-evidence-2026-06-06.md)
  proves the first Fly Rust cutover plus public/read-route smoke.
- [docs/architecture/rust-write-path-evidence-2026-06-12.md](docs/architecture/rust-write-path-evidence-2026-06-12.md)
  proves the live admin, ingest, target, monitor, webhook, delivery-ledger,
  query/report/timeline, cleanup, and DR-status write-path rehearsal.

Replay the write-path proof with `bin/canary-write-path-rehearsal --json`
against a live Canary instance; it creates uniquely named disposable resources,
redacts credentials in the receipt, then deletes or revokes the live resources
it created.

### Bootstrap

From the repo root:

```bash
./bin/bootstrap
```

That command:

- runs `npm ci` for `clients/typescript/`
- runs `cargo fetch --locked` for the Rust workspace
- configures `core.hooksPath` to use `.githooks/`

### Validation

Run the canonical repo-local quality gate from the repo root:

```bash
./bin/validate
```

`./bin/validate` defaults to the canonical Dagger check, which runs the
deterministic package gates plus the git-history secrets scan, and
automatically uses the repo-local `./bin/dagger` wrapper.

`./bin/dagger` refuses local CLI version drift so local runs match the Dagger
version pinned for CI in `dagger.json`.

On macOS, make sure the active Docker client works. If you use Colima:

```bash
colima start --runtime docker
./bin/validate
```

Use the wrapper directly when you want raw Dagger entrypoints from the repo:

```bash
./bin/dagger check
./bin/dagger call codex-agent-roles
./bin/dagger call fast
```

`dagger/scripts/sync_source_arguments.py` is the single source of truth for
the repo-source `ignore` lists on Dagger `Directory` arguments. Dagger's
TypeScript introspector still requires inline literals in `dagger/src/index.ts`,
so update the policy table and run:

```bash
python3 dagger/scripts/sync_source_arguments.py --write
```

The deterministic portion of that gate enforces checks across the maintained
packages:

- Rust workspace: format, check, clippy, tests
- TypeScript SDK: typecheck, coverage, build
- operator scripts: entrypoint, DR, and dogfood audit tests
- production image: Docker build plus `/healthz` and `/readyz` smoke

### Fleet Integration

Agents wiring another runtime into Canary should use the 15-minute recipe in
[`docs/factory-fleet-integration.md`](docs/factory-fleet-integration.md). It
covers HTTP target enrollment, Fly private-network reachability, non-HTTP
check-in monitors, and strict dogfood readback without requiring route trivia
or secret values in receipts.

The default gate also includes the git-history secrets scan. Run live
dependency advisory scans explicitly when you want current registry state as
part of a stricter local release check:

```bash
./bin/validate --advisories
```

Or run both in sequence:

```bash
./bin/validate --strict
```

`--strict` also validates local `.codex/agents/*.toml` role metadata when
present before the deterministic and advisory phases. Canary normally relies
on the globally configured Spellbook harness, so an absent repo-local role
directory is valid.

Repo-local metadata validation can also be invoked directly:

```bash
./bin/dagger call codex-agent-roles
```

The pre-commit hook runs the fast local subset instead:

```bash
./bin/dagger call fast
```

The pre-push hook runs the full Dagger gate before local pushes:

```bash
./bin/validate --strict
```

GitHub Actions mirrors that strict path through an immutable control plane: the
workflow runs in the base-branch context, checks out a trusted base snapshot
plus the candidate snapshot separately, and executes `dagger call strict
--source=../candidate` from the trusted checkout. That keeps required CI
definition outside the candidate diff while preserving the same strict Dagger
entrypoint. See [docs/ci-control-plane.md](docs/ci-control-plane.md).

### Agent MCP Server

Canary's MCP surface is the CLI contract served over stdio:

```bash
CANARY_ENDPOINT="https://<your-fly-app>.fly.dev" \
CANARY_READ_API_KEY=... \
CANARY_RESPONDER_KEY=... \
bin/canary mcp-server
```

MCP clients can list and call the same generated tools exposed by
`bin/canary mcp-manifest`: summary, services, errors, incidents, timeline,
targets, monitors, doctor, dogfood, event capture, remediation claims,
annotations, and integration helpers. Read tools use read or responder
authority. Claim and annotation writeback tools require `responder-write`
authority, or admin for break-glass operator use. The server returns MCP
`inputSchema` fields at the wire boundary while the checked CLI manifest remains gated in
`priv/mcp/canary-cli-tools.json`.

### Cold-Agent Readiness Proof

```bash
bin/canary-readiness-proof --json
```

One discoverable entrypoint proving a cold agent can inspect and operate this
instance: `doctor`, `mcp-manifest`/`mcp-server`, dogfood discovery, and
`bin/validate --fast`, ending in a redacted receipt. Missing credentials or an
unconfigured dogfood registry report as concrete blocked fields rather than
failing the proof; see `docs/agent-inspection-cli.md#cold-agent-readiness-proof`.

## API

The machine-readable contract lives at `GET /api/v1/openapi.json`. That
endpoint, `/healthz`, and `/readyz` are public. The contract embeds the
canonical agent replay guide in `info.x-agent-guide`.

Set the instance endpoint before running API examples:

```bash
export CANARY_ENDPOINT="https://<your-fly-app>.fly.dev"
```

All other endpoints require a scoped API key:

- `ingest-only` for `POST /api/v1/errors` and `POST /api/v1/check-ins`
- `read-only` for query/report/timeline-style reads
- `responder-write` for service-bound responder reads plus claim and annotation writeback
- `admin` for onboarding, key management, target/monitor/webhook management, metrics, and other operator mutations

Manual rotation steps live in [docs/api-key-rotation.md](docs/api-key-rotation.md).

### Error Ingestion

```bash
curl -X POST $CANARY_ENDPOINT/api/v1/errors \
  -H "Authorization: Bearer $CANARY_INGEST_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "service": "cadence",
    "error_class": "Ecto.NoResultsError",
    "message": "expected at least one result but got none for user 4a8f...",
    "stack_trace": "...",
    "context": {"user_id": "4a8f...", "endpoint": "/api/sessions"},
    "severity": "error"
  }'
```

Response: `201 Created`
```json
{"id": "ERR-a1b2c3", "group_hash": "sha256...", "is_new_class": true}
```

### Query Errors

```bash
# Recent errors for a service
curl "$CANARY_ENDPOINT/api/v1/query?service=cadence&window=1h" \
  -H "Authorization: Bearer $CANARY_READ_KEY"

# Error detail
curl "$CANARY_ENDPOINT/api/v1/errors/ERR-a1b2c3" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

### Health Status

```bash
curl "$CANARY_ENDPOINT/api/v1/health-status" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

Response includes natural-language summary:
```json
{
  "summary": "4 health surfaces monitored. 3 up, 1 degraded (desktop-active-timer).",
  "targets": [...],
  "monitors": [...]
}
```

### Unified Report

```bash
curl "$CANARY_ENDPOINT/api/v1/report?window=1h" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

Response combines the current health view, active error groups, recent transitions,
windowed service SLIs, and correlated incidents in one current-state payload.
Targets, monitors, and error groups are cursor-paginated; `service_sli` is a
compact per-service whole-window summary scoped by auth, window, and any API-key
service binding, and is not advanced by `report.cursor`:

```json
{
  "status": "degraded",
  "summary": "3 health surfaces monitored. 1 degraded (volume). 14 errors across 1 service in the last hour.",
  "targets": [...],
  "monitors": [...],
  "service_sli": [
    {
      "service": "volume",
      "window": "1h",
      "slo": {
        "class": "standard",
        "source": "default_health_surface",
        "availability_target": 0.995,
        "latency_ms_average_target": 1000,
        "error_budget_events_per_hour": 5
      },
      "targets": {
        "configured": 2,
        "checks": 120,
        "successful_checks": 118,
        "failed_checks": 2,
        "availability_ratio": 0.9833333333333333,
        "latency_ms_average": 84.5
      },
      "monitors": {
        "configured": 1,
        "check_ins": 12,
        "healthy_check_ins": 11,
        "failed_check_ins": 1,
        "availability_ratio": 0.9166666666666666
      },
      "errors": {
        "total": 14,
        "groups": 3
      },
      "incidents": {
        "opened": 1,
        "resolved": 0,
        "active": 1
      }
    }
  ],
  "error_groups": [...],
  "incidents": [
    {
      "id": "INC-a1b2c3",
      "service": "volume",
      "state": "investigating",
      "severity": "high",
      "signals": [...]
    }
  ],
  "recent_transitions": [...]
}
```

### Timeline

```bash
curl "$CANARY_ENDPOINT/api/v1/timeline?service=volume&window=24h&limit=50" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

Timeline events are canonical observability facts. The same payloads drive both
timeline queries and outbound webhook deliveries.

Optional free-text error search stays on the same endpoint:

```bash
curl "$CANARY_ENDPOINT/api/v1/report?window=1h&q=timeout" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

When `q` is present, the response adds `search_results`, scoped to the same
window as the rest of the report:

```json
{
  "status": "degraded",
  "summary": "3 health surfaces monitored. 1 degraded (volume). 14 errors across 1 service in the last hour.",
  "monitors": [...],
  "search_results": [
    {
      "id": "ERR-a1b2c3",
      "service": "volume",
      "error_class": "TimeoutError",
      "message": "timeout while reaching upstream",
      "group_hash": "sha256...",
      "created_at": "2026-03-24T20:15:00Z",
      "score": 1.73
    }
  ]
}
```

### Connect a Service

Use the onboarding endpoint when you want one opinionated flow that creates a
health target, generates a fresh ingest key, and returns exact copy/paste
snippets for reporting errors and verifying the service in Canary.

```bash
curl -X POST $CANARY_ENDPOINT/api/v1/service-onboarding \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"service": "my-api", "url": "https://my-api.fly.dev/health", "environment": "production"}'
```

The response includes:

- a one-time raw `ingest-only` API key for the new service
- the created target metadata
- exact snippets for `POST /api/v1/errors`, plus report/query verification commands that expect a separate read/admin key
- the unified `GET /api/v1/report` endpoint for post-onboarding verification

### Owned Service Dogfooding

Use [docs/networked-service-dogfooding.md](docs/networked-service-dogfooding.md)
and `bin/dogfood-audit --strict` to verify an instance-local deployed-service
registry against a live Canary instance. Initialize it from
`priv/dogfood/owned_services.example.json` into
`.canary/dogfood/owned_services.json` and keep production service names out of
committed examples. Add `--json` when an agent or CI job needs a
machine-readable report.

### Non-HTTP Runtime Health

Use [docs/non-http-health-semantics.md](docs/non-http-health-semantics.md)
for the decision record behind Canary's non-HTTP health model. Canary now keeps
HTTP polling for `Target`s and models desktop apps, cron jobs, and workers as
check-in monitors managed separately from URL-backed targets.

```bash
# Create a schedule-based monitor
curl -X POST $CANARY_ENDPOINT/api/v1/monitors \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "desktop-active-timer",
    "service": "time-tracker",
    "mode": "schedule",
    "expected_every_ms": 300000,
    "grace_ms": 60000
  }'

# Report a healthy check-in
curl -X POST $CANARY_ENDPOINT/api/v1/check-ins \
  -H "Authorization: Bearer $CANARY_INGEST_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "monitor": "desktop-active-timer",
    "status": "alive",
    "observed_at": "2026-06-20T18:00:00Z"
  }'
```

Healthy check-ins advance the monitor state without creating error groups.
`observed_at` is optional, but when supplied it cannot be more than five
minutes in the future relative to Canary receipt time.
Crash or exception telemetry still belongs on `POST /api/v1/errors`.

### Target Management

```bash
# Add target
curl -X POST $CANARY_ENDPOINT/api/v1/targets \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "my-api", "service": "my-api", "url": "https://my-api.fly.dev/health", "interval_ms": 60000}'

# List / pause / resume / delete
curl $CANARY_ENDPOINT/api/v1/targets -H "Authorization: Bearer $CANARY_ADMIN_KEY"
curl -X POST .../targets/:id/pause
curl -X POST .../targets/:id/resume
curl -X DELETE .../targets/:id
```

### Monitor Management

```bash
# List / create / delete monitors
curl $CANARY_ENDPOINT/api/v1/monitors -H "Authorization: Bearer $CANARY_ADMIN_KEY"
curl -X POST $CANARY_ENDPOINT/api/v1/monitors \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "desktop-active-timer", "mode": "ttl", "expected_every_ms": 60000, "grace_ms": 15000}'
curl -X DELETE .../monitors/:id
```

### Webhook Management

```bash
curl -X POST $CANARY_ENDPOINT/api/v1/webhooks \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"url": "https://example.com/hook", "events": ["health_check.down", "error.new_class"]}'

curl "$CANARY_ENDPOINT/api/v1/webhook-deliveries?webhook_id=WHK-abc123&limit=20" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

### API Key Management

```bash
curl -X POST $CANARY_ENDPOINT/api/v1/keys \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "cadence-prod", "scope": "read-only"}'

curl -X POST $CANARY_ENDPOINT/api/v1/keys \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "cadence-responder", "scope": "responder-write", "service": "cadence"}'
```

## Webhook Events

| Event | Fires When |
|-------|-----------|
| `health_check.degraded` | Target or monitor transitions to degraded |
| `health_check.down` | Target or monitor transitions to down |
| `health_check.recovered` | Target or monitor recovers to up |
| `health_check.tls_expiring` | TLS cert expires in <14 days |
| `error.new_class` | First occurrence of an error group |
| `error.regression` | Error group recurs after 24h silence |
| `incident.opened` | A service gets a new correlated incident |
| `incident.updated` | Signals are attached to an active incident |
| `incident.resolved` | All signals attached to an incident are resolved |

All webhooks are HMAC-SHA256 signed. Secret returned on subscription creation.
`POST /api/v1/webhooks/:id/test` sends a non-business `canary.ping` payload and does not write to the timeline.

**Incident severity floor:** an incident's own `severity` is only ever `medium`
or `high` -- there is no `low` incident severity tier. It is computed from the
count of currently-active correlated signals (3 or more active signals =>
`high`, otherwise `medium`) and never inherits an originating signal's own
reported severity. A lone signal reported with severity `low` (see the
`incident.opened`/`incident.updated` payload's `signal.severity` field) still
opens or updates an incident at severity `medium`. See the OpenAPI `Incident`
schema (`priv/openapi/openapi.json`) for the authoritative contract.

### Webhook Consumer Contract

- Deliveries are at-least-once. Deduplicate on `X-Delivery-Id`.
- `X-Delivery-Id` is stable across retries for the same logical delivery.
- Webhooks are wake-up hints, not the source of truth. Query `/api/v1/timeline` or the relevant read API for correctness.
- `GET /api/v1/webhook-deliveries` exposes operator-visible delivery outcomes, attempt counts, reasons, and cursor-paginated history.

## Self-Observability

| Endpoint | Purpose |
|----------|---------|
| `GET /healthz` | Liveness — HTTP router alive |
| `GET /readyz` | Readiness — DB + supervisor healthy |

## Deployment

Deploy to Fly.io with SQLite persistence and a Fly Tigris-backed Litestream
restore path. A full cold-operator runbook lives in
[docs/self-host-fly.md](docs/self-host-fly.md).

```bash
export CANARY_FLY_APP="<your-fly-app>"
export CANARY_ENDPOINT="https://${CANARY_FLY_APP}.fly.dev"
mkdir -p .canary/dogfood
cp priv/dogfood/owned_services.example.json .canary/dogfood/owned_services.json
flyctl deploy --app "$CANARY_FLY_APP" --remote-only
```

The checked-in `fly.toml` uses a placeholder app name. Always pass
`--app "$CANARY_FLY_APP"` so a fork or clean-room checkout cannot deploy to the
Misty Step instance by accident.

On first boot, capture the one-time bootstrap admin key from Fly logs and store
it in the operator's secret manager:

```bash
flyctl logs --app "$CANARY_FLY_APP" --no-tail | grep -E 'Bootstrap API key:'
```

If the first boot log was missed, mint a replacement admin key without data
loss. This prints a raw admin key once:

```bash
flyctl ssh console --app "$CANARY_FLY_APP" \
  -C '/app/bin/canary-server mint-key --scope admin --name operator-recovery'
```

After the key is stored, prove the new instance with the same smoke commands an
agent will use later:

```bash
export CANARY_ADMIN_KEY="<stored-admin-key>"
curl -fsS "$CANARY_ENDPOINT/healthz"
curl -fsS "$CANARY_ENDPOINT/readyz"
curl -fsS "$CANARY_ENDPOINT/api/v1/report?window=1h" \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY"
bin/canary-write-path-rehearsal \
  --endpoint "$CANARY_ENDPOINT" \
  --api-key "$CANARY_ADMIN_KEY" \
  --app "$CANARY_FLY_APP" \
  --json
```

DR verification and restore procedures live in [docs/backup-restore-dr.md](docs/backup-restore-dr.md).
Use `bin/dr-status` for a read-only Litestream preflight and
`bin/dr-restore-check` for a non-destructive restore drill against the running
Fly app.
On a fresh Fly app, enable the same path with
`flyctl storage create --app "$CANARY_FLY_APP" --name "$CANARY_FLY_APP-backups" --yes`, then
re-run the two verification commands. See the DR runbook for the latest live
verification status.

See `fly.toml`, `Dockerfile`, `litestream.yml`, and `bin/entrypoint.sh`.

## Tech Stack

- **Rust** — Typed service core, Axum HTTP runtime, deterministic workers, and compile-time guardrails
- **SQLite** — WAL mode with one explicit writer boundary in `canary-store`
- **Reqwest** — HTTP target probes and outbound webhook delivery
- **Litestream + Fly Tigris** — Continuous SQLite replication to Fly-managed object storage

## License

MIT
