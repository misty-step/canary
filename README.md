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

## Quick Start

```bash
git clone https://github.com/misty-step/canary && cd canary
cp .env.example .env
./bin/bootstrap    # installs deps for the core service and SDK packages
mix phx.server     # starts on localhost:4000
```

No Docker required for local development. The repo also includes Elixir and
TypeScript SDK packages.

The operator console lives at `/dashboard`. Set `DASHBOARD_PASSWORD` to require
login in non-local environments; leave it unset in dev/test to keep the
dashboard open.

## Development

This is a monorepo with three maintained packages:

- `.` — Canary core service
- `canary_sdk/` — Elixir SDK
- `clients/typescript/` — TypeScript SDK

### Toolchain

Supported local toolchains are pinned in `.tool-versions`:

- Erlang/OTP `27.3.4.9`
- Elixir `1.17.3-otp-27`
- Node.js `22.22.0`

Local validation also requires the `dagger` CLI, pinned to the version declared
in `dagger.json`. On macOS, repo-local Dagger uses the active local Docker
client first and falls back to Colima-over-SSH if direct Docker access is
unavailable. GitHub Actions and git hooks delegate to the same pinned Dagger
surface.

The production Dockerfile also builds on Elixir `1.17`, and CI uses the same pinned toolchain versions.

### Bootstrap

From the repo root:

```bash
./bin/bootstrap
```

That command:

- runs `mix setup` for the core service
- installs `canary_sdk/` dependencies
- runs `npm ci` for `clients/typescript/`
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

- core: compile, format, credo, sobelow, coverage, dialyzer
- Elixir SDK: compile, format, coverage
- TypeScript SDK: typecheck, coverage, build

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

`--strict` also includes the local `.codex/agents/*.toml` role validation
before the deterministic and advisory phases.

Repo-local metadata validation for `.codex/agents/*.toml` is part of the local
hook surfaces and can also be invoked directly:

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

## API

The machine-readable contract lives at `GET /api/v1/openapi.json`. That
endpoint, `/healthz`, and `/readyz` are public. The contract embeds the
canonical agent replay guide in `info.x-agent-guide`.

All other endpoints require a scoped API key:

- `ingest-only` for `POST /api/v1/errors`
- `read-only` for query/report/timeline-style reads
- `admin` for onboarding, key management, target/webhook management, metrics, and other operator mutations

Manual rotation steps live in [docs/api-key-rotation.md](docs/api-key-rotation.md).

### Error Ingestion

```bash
curl -X POST https://canary-obs.fly.dev/api/v1/errors \
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
curl "https://canary-obs.fly.dev/api/v1/query?service=cadence&window=1h" \
  -H "Authorization: Bearer $CANARY_READ_KEY"

# Error detail
curl "https://canary-obs.fly.dev/api/v1/errors/ERR-a1b2c3" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

### Health Status

```bash
curl "https://canary-obs.fly.dev/api/v1/health-status" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

Response includes natural-language summary:
```json
{
  "summary": "3 targets monitored. 2 up, 1 degraded (cadence-api: 2 consecutive failures).",
  "targets": [...]
}
```

### Unified Report

```bash
curl "https://canary-obs.fly.dev/api/v1/report?window=1h" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

Response combines the current health view, active error groups, recent transitions,
and correlated incidents in one bounded payload:

```json
{
  "status": "degraded",
  "summary": "2 targets monitored. 1 degraded (volume). 14 errors across 1 service in the last hour.",
  "targets": [...],
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
curl "https://canary-obs.fly.dev/api/v1/timeline?service=volume&window=24h&limit=50" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

Timeline events are canonical observability facts. The same payloads drive both
timeline queries and outbound webhook deliveries.

Optional free-text error search stays on the same endpoint:

```bash
curl "https://canary-obs.fly.dev/api/v1/report?window=1h&q=timeout" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

When `q` is present, the response adds `search_results`, scoped to the same
window as the rest of the report:

```json
{
  "status": "degraded",
  "summary": "2 targets monitored. 1 degraded (volume). 14 errors across 1 service in the last hour.",
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
curl -X POST https://canary-obs.fly.dev/api/v1/service-onboarding \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"service": "my-api", "url": "https://my-api.fly.dev/health", "environment": "production"}'
```

The response includes:

- a one-time raw `ingest-only` API key for the new service
- the created target metadata
- exact snippets for `POST /api/v1/errors`, plus report/query verification commands that expect a separate read/admin key
- direct links to `/dashboard` and the unified report

### Owned Service Dogfooding

Use [docs/networked-service-dogfooding.md](/Users/phaedrus/Development/canary/docs/networked-service-dogfooding.md)
and `bin/dogfood-audit --strict` to verify the checked-in owned HTTP service
set against a live Canary instance.

### Non-HTTP Runtime Health

Use [docs/non-http-health-semantics.md](/Users/phaedrus/Development/canary/docs/non-http-health-semantics.md)
for the selected health model for desktop apps, cron jobs, and workers. The
short version: keep HTTP polling for `Target`s, and model non-HTTP runtimes as
future check-in monitors instead of inventing fake URLs or overloading error
ingest.

### Target Management

```bash
# Add target
curl -X POST https://canary-obs.fly.dev/api/v1/targets \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "my-api", "service": "my-api", "url": "https://my-api.fly.dev/health", "interval_ms": 60000}'

# List / pause / resume / delete
curl https://canary-obs.fly.dev/api/v1/targets -H "Authorization: Bearer $CANARY_ADMIN_KEY"
curl -X POST .../targets/:id/pause
curl -X POST .../targets/:id/resume
curl -X DELETE .../targets/:id
```

### Webhook Management

```bash
curl -X POST https://canary-obs.fly.dev/api/v1/webhooks \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"url": "https://example.com/hook", "events": ["health_check.down", "error.new_class"]}'

curl "https://canary-obs.fly.dev/api/v1/webhook-deliveries?webhook_id=WHK-abc123&limit=20" \
  -H "Authorization: Bearer $CANARY_READ_KEY"
```

### API Key Management

```bash
curl -X POST https://canary-obs.fly.dev/api/v1/keys \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "cadence-prod", "scope": "read-only"}'
```

## Webhook Events

| Event | Fires When |
|-------|-----------|
| `health_check.degraded` | Target transitions to degraded |
| `health_check.down` | Target transitions to down |
| `health_check.recovered` | Target recovers to up |
| `health_check.tls_expiring` | TLS cert expires in <14 days |
| `error.new_class` | First occurrence of an error group |
| `error.regression` | Error group recurs after 24h silence |
| `incident.opened` | A service gets a new correlated incident |
| `incident.updated` | Signals are attached to an active incident |
| `incident.resolved` | All signals attached to an incident are resolved |

All webhooks are HMAC-SHA256 signed. Secret returned on subscription creation.
`POST /api/v1/webhooks/:id/test` sends a non-business `canary.ping` payload and does not write to the timeline.

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

Deployed to Fly.io with SQLite persistence and a Fly Tigris-backed Litestream
restore path.

```bash
flyctl deploy --app canary-obs
```

DR verification and restore procedures live in [docs/backup-restore-dr.md](docs/backup-restore-dr.md).
Use `bin/dr-status` for a read-only Litestream preflight and
`bin/dr-restore-check` for a non-destructive restore drill against the running
Fly app.
On a fresh Fly app, enable the same path with
`flyctl storage create --app canary-obs --name canary-obs-backups --yes`, then
re-run the two verification commands. See the DR runbook for the latest live
verification status.

See `fly.toml`, `Dockerfile`, `litestream.yml`, and `bin/entrypoint.sh`.

## Tech Stack

- **Elixir/OTP** — GenServer-per-target health checkers, DynamicSupervisor, crash isolation
- **Phoenix** — HTTP routing, plug pipeline, telemetry, and a thin LiveView operator console
- **SQLite** — WAL mode, write-serialized, Ecto abstraction preserves Postgres migration path
- **Oban** — Webhook delivery retries, retention pruning, TLS scanning
- **Req/Finch** — Connection-pooled HTTP probes
- **Litestream + Fly Tigris** — Continuous SQLite replication to Fly-managed object storage

## License

MIT
