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
bin/canary integrate discover /path/to/app --production-url https://app.example.com --json
bin/canary integrate plan /path/to/app --service app --production-url https://app.example.com --json
bin/canary integrate patch /path/to/app --service app --json
bin/canary integrate enroll --service app --url https://app.example.com/api/health --json
bin/canary doctor --json
bin/canary mcp-manifest
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

`doctor` is the fastest "who watches Canary?" command. It probes `/healthz`,
`/readyz`, the global report, service status, incidents, dogfood coverage,
recent `service=canary` errors, worker lifecycle readiness, and the external
`canary-watchman` monitor created by `bin/canary-witness`. The worker readiness
line is derived from `/readyz` and should look like:

```text
worker_readiness: ready 5 workers, 0 failing
```

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

## Integration Agent

`integrate` is the agent-native setup loop for deployed applications. It is
deliberately split into reviewable phases:

- `discover <path-or-project> --json` reads local project structure, framework hints,
  Vercel/Fly markers, package dependencies, existing Sentry/Canary code paths,
  health routes, and environment variable names. It does not read or print env
  values.
- `plan <path-or-project> --json` emits an action list for SDK instrumentation, health
  routes, platform env names, Canary target enrollment, monitor/webhook followup,
  and the commands the agent should run next.
- `patch <path-or-project> --json` applies the safe Next.js code path: adds
  `@canary-obs/sdk`, `instrumentation.ts`, `app/api/health/route.ts`, and
  `app/global-error.tsx` only when files are absent or already Canary-owned.
- `enroll --service <name> --url <health-url> --json` calls the admin API to
  create the health target and scoped ingest key. The one-time key and snippets
  are redacted unless `--show-secret` is explicitly passed for a secure handoff.

Use SDK instrumentation when the application code can ship a patch: it provides
service names, environments, scrubbed context, and typed request/browser error
capture. Use platform-level drains only as a supplement for logs/errors the app
cannot reach directly; drains do not replace typed SDK context and still need
Canary target/query readback before coverage is claimed.

For Vercel projects, pair `integrate discover/plan` with `vercel env ls
production --cwd <repo> --format=json` and `vercel env ls preview --cwd <repo>
--format=json` to audit env-name presence without exposing values. For Fly apps,
pair it with `flyctl status` and the app's public health URL. Agents should not
write platform secrets unless the value arrives through stdin or an approved
secret-manager handoff.

## MCP Shape

`bin/canary mcp-manifest` emits the generated tool manifest. MCP tools should shell
out to the CLI and reuse the same JSON schemas rather than reimplementing route
semantics.
