# Backlog Ideas

Last groomed: 2026-03-15

## High Potential (promote next session if capacity)

- **Sentry removal from volume** — After dual-write validation (1 week), cut `@sentry/nextjs`. Blocked on #29 completing. Source: groom session 2026-03-15.
- **Sentry watcher migration in bitterblossom** — Replace `sentry-watcher.sh` polling with Canary query API. Blocked on #28 completing. Source: codebase exploration.
- **Reference clients (Python, Go)** — Needed for non-Elixir, non-TS services. Low urgency until more services exist. Source: project.md.

## Someday / Maybe

- **Elixir client library (standalone)** — `clients/elixir/canary_client.ex` exists (108 LOC) but not a proper Hex package. May merge with SDK or keep separate. Source: project.md.
- **Webhook replay/retry dashboard** — Visibility into webhook delivery state beyond Oban jobs table. May be absorbed by #47 (LiveView dashboard) in a future iteration. Source: quality audit.
- **Metrics export (Prometheus/StatsD)** — Telemetry module exists but only defines reporters, no custom metrics. Source: quality audit.
- **API key role scoping** — Admin vs read-only keys. Currently all keys are equivalent. Source: quality audit.
- **Heartbeat monitors (passive)** — Services ping Canary on a schedule; alert if they stop. Good for cron jobs and workers. Source: SDK research.
- **MCP server** — If Canary needs to be accessible from Cursor, Claude Desktop, or other MCP hosts beyond Claude Code. SKILL.md (#44) is the blueprint. Source: groom session 2026-03-15 (deferred in favor of skill file).

## Research Prompts

- **Litestream S3 provider** — DO Spaces vs Tigris vs Cloudflare R2 for SQLite replication. Cost, latency from iad.
- **Hex package CI** — How to publish Hex packages from a monorepo GitHub Actions workflow.

## Archived This Session

- ~~Canary DSN connection string~~ — Deferred indefinitely. Three env vars (endpoint + api_key + service) are more debuggable than a custom URL scheme. Premature abstraction for single-user.
- ~~Claude Code skill file~~ — Promoted to #44.
- ~~Status page~~ — Out of scope for agent-first product. Dashboard (#47) serves operators; no public-facing status page needed.
