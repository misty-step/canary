# Backlog Ideas

Last groomed: 2026-03-22

## High Potential (promote next session if capacity)

- **Sentry removal from volume** — #29 (dual-write) is complete. Ready to cut `@sentry/nextjs` after 1 week validation. Source: groom 2026-03-15, unblocked 2026-03-20.
- **Desktop health semantics** — What does health monitoring mean for non-HTTP apps (Electron, CLI, cron)? Heartbeat, relay, companion process? Demoted from #71. Source: groom 2026-03-22.

## Someday / Maybe

- **MCP server** — Expose incidents, errors, health as MCP tools for Cursor, Claude Desktop, etc. Deferred — fix the API surface first. Source: groom 2026-03-22 (reference search confirmed MCP is table stakes by mid-2026).
- **Heartbeat monitors (passive)** — Services ping Canary on a schedule; alert if they stop. Good for cron jobs, workers, desktop apps. Source: SDK research.
- **API key role scoping** — Admin vs read-only keys. Currently all keys are equivalent. Source: quality audit.
- **Elixir client library (standalone)** — `clients/elixir/canary_client.ex` exists (108 LOC) but not a proper Hex package. Source: project.md.
- **Webhook replay/retry dashboard** — Visibility into webhook delivery state. Dashboard (#47) shipped; this would be a future iteration. Source: quality audit.
- **Metrics export (Prometheus/StatsD)** — Telemetry module exists but only defines reporters. Source: quality audit.
- **OpenTelemetry ingest** — Accept OTel spans/logs alongside native ingest. GenAI semantic conventions are stabilizing. Source: reference search 2026-03-22.
- **Natural language → structured query** — Sentry and Logfire both do this. Could use local LLM or simple NLU over FTS5. Depends on #76 (FTS5). Source: reference search 2026-03-22.

## Research Prompts

- **Litestream S3 provider** — DO Spaces vs Tigris vs Cloudflare R2 for SQLite replication. Cost, latency from iad.
- **Hex package CI** — How to publish Hex packages from a monorepo GitHub Actions workflow.
- **Token-based pagination patterns** — Datadog paginates by token budget, not record count. How to implement with SQLite cursors? Source: reference search 2026-03-22.

## Archived This Session (2026-03-22)

- ~~Desktop health semantics (#71)~~ — Demoted from GitHub to high-potential. Research question, not execution-ready.

## Archived (2026-03-20)

- ~~Sentry watcher migration in bitterblossom~~ — Promoted to #69.
- ~~Reference clients (Python, Go)~~ — API-first approach: the ingest API is 3 fields, no language-specific SDK needed. Any HTTP client works.

## Archived (2026-03-15)

- ~~Canary DSN connection string~~ — Deferred indefinitely. Three env vars are more debuggable than a custom URL scheme.
- ~~Claude Code skill file~~ — Promoted to #44.
- ~~Status page~~ — Out of scope for agent-first product.
