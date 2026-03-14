# CLAUDE.md

## What This Is

Canary — self-hosted observability for agent-driven infrastructure. Elixir/Phoenix + SQLite.

## Commands

```bash
mix setup                    # deps.get + ecto.create + ecto.migrate
mix phx.server               # start on localhost:4000
mix test                     # run all tests
mix compile --warnings-as-errors  # strict compile
```

## Architecture

Single OTP application. Three subsystems:

1. **Health checking** — GenServer-per-target under DynamicSupervisor. State machine: unknown → up → degraded → down. SSRF-guarded probes via shared Finch pool.
2. **Error ingestion** — POST → validate → group (3 strategies: fingerprint, stack trace, message template) → persist (errors + error_groups upsert) → webhook.
3. **Webhook broadcasting** — Oban workers with HMAC signing, circuit breaker, cooldown. At-least-once delivery.

## Key Invariants

- `Canary.Repo` pool_size: 1 (SQLite single-writer). All writes go through this.
- `Canary.ReadRepo` pool_size: 4. All query API reads go through this.
- `StateMachine.transition/4` is a pure function. No side effects. Test it with table-driven tests.
- Summary generation is deterministic template strings. No LLM calls.
- All error responses use RFC 9457 Problem Details.

## File Organization

- `lib/canary/health/` — health checking (GenServers, state machine, probes, SSRF)
- `lib/canary/errors/` — error ingest (grouping, rate limiting, dedup)
- `lib/canary/alerter/` — webhook delivery (signing, circuit breaker, cooldown)
- `lib/canary/workers/` — Oban background jobs
- `lib/canary/schemas/` — Ecto schemas
- `lib/canary_web/` — Phoenix router, plugs, controllers

## Testing

```bash
mix test                           # all 54 tests
mix test test/canary/health/       # health subsystem
mix test test/canary/errors/       # error subsystem
```

- Pure function tests (StateMachine, Grouping, Summary) are async
- DB tests (Auth, Ingest) use Ecto sandbox, sync
- Health.Manager retries gracefully in test (sandbox ownership)

## Deployment

Fly.io app: `canary-obs`, region: `iad`. SQLite at `/data/canary.db`.

```bash
flyctl deploy --app canary-obs --remote-only
flyctl logs --app canary-obs --no-tail
```

## Conventions

- IDs: `ERR-<nanoid>`, `TGT-<nanoid>`, `WHK-<nanoid>`, `KEY-<nanoid>`
- Timestamps: ISO 8601 strings (SQLite TEXT columns)
- Primary keys: set on struct (`%Error{id: id}`), not via changeset cast
- Config: DB is canonical runtime config. Seeds bootstrap on first boot only.
