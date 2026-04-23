# CLAUDE

Self-hosted observability for agent-driven infrastructure. Elixir/Phoenix + SQLite.

## Monorepo Layout

- Root: **Canary** core service (error ingestion, health checking, webhooks). Fly app: `canary-obs`.
- External responders consume Canary's signed webhooks and query APIs. They are not part of this repo.

## Footguns

- **Ecto primary keys.** Custom string PKs (`ERR-nanoid`) must be set on the struct, not cast: `%Error{id: id} |> changeset(attrs)`. Casting silently drops the `id` field because it's not in the `@required`/`@optional` lists. Enforced by `Canary.Checks.EctoPKViaCast` in `./bin/validate --fast`. (6 bugs in initial build.)
- **Oban Lite tables.** Oban's SQLite engine does NOT auto-create its tables. `Oban.Migrations.SQLite` exists but `Ecto.Migrator.run` can't invoke it (different migration behaviour). Fixed: dedicated Ecto migration (`20260314230000_create_oban_jobs.exs`) with `execute` for raw SQL. Do NOT put this in a GenServer or Release module — `Repo.query!` races with pool_size:1 during Ecto migration.
- **Req + Finch.** Cannot pass both `:finch` and `:connect_options` to `Req.request/1`. The `:finch` option implies connection management — use `:receive_timeout` for timeouts.
- **ReadRepo is not in `ecto_repos`.** Only `Canary.Repo` runs migrations. Adding `ReadRepo` to `ecto_repos` makes `mix ecto.migrate` look for `priv/read_repo/migrations/` which doesn't exist.
- **Fly.io port binding.** The prod endpoint config must explicitly include `port:` in the `http:` keyword list. A second `config :canary, CanaryWeb.Endpoint` block in `runtime.exs` replaces (not merges) the `http:` key — omitting `port:` causes random port binding.
- **Health.Manager boot resilience.** Uses `rescue` in `handle_info(:boot)` to retry in 5s if DB isn't ready. Required because in test mode (Ecto sandbox) and during production boot races, the targets table may not exist yet.
- **SQLite WAL and `rm -f`.** Deleting the DB while the app is running does nothing — SQLite WAL keeps the file handle open. Must stop the machine first, then SSH in to delete, then restart.

## Invariants

- `Canary.Repo` pool_size: 1. SQLite single-writer. All writes through this.
- `StateMachine.transition/4` is pure. No side effects. Table-driven tests.
- Summary generation is deterministic templates. No LLM on request path.
- RFC 9457 Problem Details for all error responses.
- No service names hardcoded. Targets/webhooks configured at runtime via API.
- Seeds only create a bootstrap API key. No hardcoded targets.

## Deploy

```bash
# Core service (from repo root)
flyctl deploy --app canary-obs --remote-only

# Nuclear reset (stop first, then delete, then restart)
flyctl machines stop <id> --app canary-obs
flyctl machines start <id> --app canary-obs
flyctl ssh console --app canary-obs -C "rm -f /data/canary.db /data/canary.db-wal /data/canary.db-shm"
flyctl machines restart <id> --app canary-obs
```

Bootstrap API key logged on first boot — grep for `"Bootstrap API key:"`. Store it; it won't be shown again.

## Responder Boundary

Canary is the observability substrate. It owns error ingest, health checks,
incident correlation, timelines, query APIs, and signed generic webhooks.

- Repo mutation, issue creation, and LLM triage live outside Canary.
- Consumers should subscribe via generic webhooks and query back into Canary for context.
- Treat webhook payloads as stable product contracts, not app-specific glue.
- Circuit breaker opens after 10 failures, probes every 5 min. Cooldown is 5 min per webhook+event type. Restart canary-obs to reset ETS state.

## Self-Monitoring

Canary reports its own errors via `:logger` handlers:
- Core (`Canary.ErrorReporter`): direct ingest — no HTTP, calls `Canary.Errors.Ingest.ingest/1` directly.
