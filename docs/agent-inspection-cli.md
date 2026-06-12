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
recent `service=canary` errors, and the external `canary-watchman` monitor
created by `bin/canary-witness`. The witness line is:

- `observed`: the scheduled witness has checked in; the line includes the last
  check-in status and timestamp.
- `configured`: the monitor exists but status readback has not observed a
  check-in yet.
- `missing`: the `canary-watchman` monitor is not configured.
- `unavailable`: the CLI could not inspect monitor/status routes with the
  supplied key.

The witness runbook and GitHub Actions receipt contract live in
`docs/canary-witness.md`.

## MCP Shape

`bin/canary mcp-manifest` emits the generated tool manifest. MCP tools should shell
out to the CLI and reuse the same JSON schemas rather than reimplementing route
semantics.
