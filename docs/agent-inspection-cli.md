# Agent Inspection CLI

Canary exposes an agent-first local inspection surface through the Rust
`canary` binary. Inside this repo, agents should use `bin/canary`; installed
consumers can call the compiled `canary` binary directly. The CLI is
intentionally a thin adapter over Canary's HTTP API and existing dogfood
inventory command; it does not create a second semantic API.

## Configuration

The CLI reads:

- `CANARY_ENDPOINT`, defaulting to `https://canary-obs.fly.dev`
- `CANARY_ADMIN_API_KEY`, `CANARY_ADMIN_KEY`, `CANARY_READ_API_KEY`,
  `CANARY_READ_KEY`, or `CANARY_API_KEY`
- `CANARY_CONFIG`, or `~/.config/canary/config.json`

Config JSON:

```json
{
  "endpoint": "https://canary-obs.fly.dev",
  "admin_api_key": "sk_live_...",
  "read_api_key": "sk_live_..."
}
```

Admin keys are preferred over read keys when both are present because target
and monitor inspection use admin-scoped routes. Scoped read/admin credentials
from env or config are preferred over generic `CANARY_API_KEY` so app ingest
keys do not shadow operator credentials. Diagnostics redact keys and fail
closed when an authenticated command is run without a read/admin key.

## Commands

```bash
bin/canary summary --window 24h
bin/canary services --state down --json
bin/canary errors chrondle --window 24h
bin/canary incidents --open
bin/canary timeline --service chrondle --window 7d --limit 20
bin/canary targets
bin/canary monitors
bin/canary dogfood audit --strict
bin/canary dogfood value --service linejam --json
bin/canary integrate discover /path/to/app --production-url https://app.example.com --json
bin/canary integrate plan /path/to/app --service app --production-url https://app.example.com --json
bin/canary integrate patch /path/to/app --service app --json
bin/canary integrate enroll --service app --url https://app.example.com/api/health --project-root /path/to/app --json
bin/canary doctor --json
bin/canary mcp-manifest
bin/canary mcp-server
```

Every command supports `--json`. JSON output is wrapped in a stable envelope:

```json
{
  "schema_version": 1,
  "command": "summary",
  "endpoint": "https://canary-obs.fly.dev",
  "response": {}
}
```

Text output is deliberately compact for agent transcripts.

`dogfood audit --strict --json` still prints the JSON report before exiting
nonzero when coverage gaps remain, so agents can inspect the failure details.

`dogfood value --service <name> --json` builds a per-service value receipt from
the dogfood inventory plus live Canary readback: coverage verdict,
`/api/v1/status` target or monitor health, recent error and incident counts,
active remediation claim, recent annotations, telemetry events, synthetic
verification status, and one next action. Use it when an agent needs to answer
"what did Canary prove for this service?" rather than "is the service
registered?"

`doctor` is the fastest "who watches Canary?" command. It probes `/healthz`,
`/readyz`, the global report, service status, incidents, dogfood coverage,
recent `service=canary` errors, worker lifecycle readiness, and the external
`canary-watchman` monitor created by `bin/canary-witness`.
`dogfood_value` counts are diagnostic buckets, not a partition: `covered`,
`blocked`, `partial`, and `ignored` come from the dogfood inventory summary,
while `stale` and `value_unproven` are recomputed from per-service receipt
fields and may overlap with coverage states.

The doctor envelope's `response` object includes an operator verdict:

```json
{
  "verdict": {
    "overall": "degraded",
    "blocking_signals": [
      "canary-watchman down; last alive check-in was 720000 ms ago"
    ],
    "next_operator_action": "Run `gh workflow run \"Canary Witness\" --ref master`; then inspect the latest witness receipt and rerun `bin/canary doctor --json`.",
    "witness_age_ms": 720000,
    "open_canary_incident": {
      "id": "INC-example",
      "service": "canary",
      "state": "open"
    },
    "worker_pressure": {
      "status": "ready",
      "pressured_workers": 0,
      "failing_workers": 0,
      "workers": []
    },
    "alert_plane": {
      "available": true,
      "status": "healthy",
      "worker_count": 5,
      "impaired_workers": 0,
      "workers": [],
      "reasons": []
    },
    "dogfood_gap_count": 3,
    "receipt_run_references": {
      "ok": true,
      "workflow": "Canary Witness",
      "runs": []
    }
  },
  "dogfood_value": {
    "ok": true,
    "response": {
      "covered": 4,
      "stale": 14,
      "blocked": 5,
      "partial": 33,
      "value_unproven": 42
    }
  }
}
```

`overall` is `healthy` only when the public routes are reachable, authenticated
readback works, the external witness is observed as up, no `service=canary`
incident is open, and `alert_plane.status` is `healthy`. `degraded` means an
agent can still inspect Canary but has work to do. `unable` means Canary cannot
produce enough authenticated self-watch evidence to trust the rest of the
report. Dogfood gaps are counted in the verdict but are not by themselves a
runtime blocker.

The worker readiness line is derived from `/readyz` and should look like:

```text
worker_readiness: ready 5 workers, 0 failing
alert_plane: healthy 5 workers
```

If a worker reports `health: pressured`, the route can still be ready. Doctor
reports that as impaired alert-plane health rather than counting the same
worker as both route-failing and healthy:

```text
worker_readiness: ready 5 workers, 0 failing, 1 pressured
alert_plane: impaired 1 worker: monitor_overdue pressured
```

`doctor` also surfaces DR evidence:

```text
dr: litestream ok, restore_receipt_missing: no architecture DR receipt found, fallback=docs/backup-restore-dr.md
```

The `dr` line is data, not a request-path dependency. It runs the operator
`bin/dr-status --app canary-obs` check when available and points to the latest
checked-in restore-specific receipt when one exists. Production startup can be
made fail-closed on backup configuration with `CANARY_REQUIRE_LITESTREAM=1`.

The witness line is:

- `observed`: the scheduled witness has checked in; the line includes the last
  check-in status and timestamp.
- `configured`: the monitor exists but status readback has not observed a
  check-in yet.
- `missing`: the `canary-watchman` monitor is not configured.
- `unavailable`: the CLI could not inspect monitor/status routes with the
  supplied key.

The witness runbook and GitHub Actions receipt contract live in
`docs/canary-witness.md`.

Doctor discovers the latest GitHub Actions witness runs with:

```bash
gh run list --workflow "Canary Witness" --branch master --limit 3 --json databaseId,status,conclusion,createdAt,updatedAt,url,event,workflowName
```

If the GitHub CLI or auth is unavailable, the verdict still returns the
replacement command plus the receipt artifact convention
`canary-witness-<run_id>`.

## Integration Agent

`integrate` is the agent-native setup loop for deployed applications. It is
deliberately split into reviewable phases:

- `discover <path-or-project> --json` reads local project structure, framework hints,
  Vercel/Fly markers, package dependencies, existing Sentry/Canary code paths,
  health routes including `src/app/**/api/health/route.ts`, integration
  receipts, and environment variable names from local env declarations plus
  receipts. It does not read or print env values.
- `status <path-or-project> --json` is the authoritative coverage read: it
  merges local discovery, `.canary/integration.json`, live targets, monitors,
  webhooks, `service` query readback, and dogfood registry evidence into one
  `coverage.status` verdict.
- `plan <path-or-project> --json` emits an action list for SDK instrumentation,
  health routes or static-site target enrollment, platform env names, receipt
  updates, Canary target enrollment, monitor/webhook follow-up, static-site
  no-code/low-code artifacts, non-HTTP monitor/check-in templates, and the
  commands the agent should run next.
- `patch <path-or-project> --json` applies the safe Next.js code path: adds
  `@canary-obs/sdk`, `instrumentation.ts`, `app/api/health/route.ts`, and
  `app/global-error.tsx` only when files are absent or already Canary-owned, and
  writes a planned `.canary/integration.json` with reviewable verification
  commands.
- `enroll --service <name> --url <health-url> --project-root <path> --json`
  calls the admin API to create the health target and scoped ingest key, then
  updates `.canary/integration.json` to verified receipt state when
  `--project-root` is supplied. The one-time key and snippets are redacted
  unless `--show-secret` is explicitly passed for a secure handoff.

Use SDK instrumentation when the application code can ship a patch: it provides
service names, environments, scrubbed context, and typed request/browser error
capture. Use platform-level drains only as a supplement for logs/errors the app
cannot reach directly; drains do not replace typed SDK context and still need
Canary target/query readback before coverage is claimed.

For Vercel projects, pair `integrate status/plan` with `vercel env ls
production --cwd <repo> --format=json` and `vercel env ls preview --cwd <repo>
--format=json` to audit env-name presence without exposing values. For Fly apps,
pair it with `flyctl status` and the app's public health URL. Agents should not
write platform secrets unless the value arrives through stdin or an approved
secret-manager handoff.

## MCP Shape

`bin/canary mcp-server` runs the generated CLI tool surface as a stdio MCP
server. MCP clients should configure the command with the same environment as
the CLI:

```json
{
  "command": "/path/to/canary",
  "args": ["mcp-server"],
  "env": {
    "CANARY_ENDPOINT": "https://canary-obs.fly.dev",
    "CANARY_READ_API_KEY": "redacted"
  }
}
```

The server supports the MCP lifecycle and tool methods an agent needs for
Canary inspection:

- `initialize`
- `notifications/initialized`
- `ping`
- `tools/list`
- `tools/call`

`tools/list` is translated from the same generated CLI manifest but uses the
MCP wire field `inputSchema`. Tool call results return a text content block and
the CLI JSON envelope as `structuredContent`. Runtime failures, such as missing
credentials or a Canary HTTP error, are returned as MCP tool results with
`isError: true` so agents can self-correct without confusing tool failures with
protocol failures.

`bin/canary mcp-manifest` still emits the checked CLI manifest snapshot shape
with `input_schema`. The checked-in snapshot at `priv/mcp/canary-cli-tools.json`
is gated against `tool_manifest()` so it cannot drift from the runtime list.

The manifest covers the drill-down surfaces an agent needs after
`canary_doctor`: summary, services, errors, incidents, timeline, targets,
monitors, dogfood audit, dogfood value receipts, witness, DR status, event
capture, remediation claims, and integration discovery/status/plan/patch/enroll.
The MCP server implements those tools directly through the CLI-backed adapter;
it does not define separate route semantics.
