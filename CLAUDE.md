# CLAUDE

Self-hosted observability for agent-driven infrastructure. Elixir/Phoenix + SQLite.

## Footguns

- **Ecto primary keys.** Custom string PKs (`ERR-nanoid`) must be set on the struct, not cast: `%Error{id: id} |> changeset(attrs)`. Casting silently drops the `id` field because it's not in the `@required`/`@optional` lists. (6 bugs in initial build.)
- **Oban Lite tables.** Oban's SQLite engine does NOT auto-create its tables. The `Release` GenServer creates them via raw SQL before Oban starts. `Oban.Migrations.SQLite` exists but `Ecto.Migrator.run` can't invoke it (different migration behaviour). **KNOWN BUG:** The current raw SQL creation races with Oban startup — the `Release` GenServer's `Repo.query!` contends with the pool_size:1 connection that Ecto migrations are using. Needs fix: either bump pool_size during boot or create tables outside the Ecto pool.
- **Req + Finch.** Cannot pass both `:finch` and `:connect_options` to `Req.request/1`. The `:finch` option implies connection management — use `:receive_timeout` for timeouts.
- **ReadRepo is not in `ecto_repos`.** Only `Canary.Repo` runs migrations. Adding `ReadRepo` to `ecto_repos` makes `mix ecto.migrate` look for `priv/read_repo/migrations/` which doesn't exist.
- **Fly.io port binding.** The prod endpoint config must explicitly include `port:` in the `http:` keyword list. A second `config :canary, CanaryWeb.Endpoint` block in `runtime.exs` replaces (not merges) the `http:` key — omitting `port:` causes random port binding.
- **Health.Manager boot resilience.** Uses `rescue` in `handle_info(:boot)` to retry in 5s if DB isn't ready. Required because in test mode (Ecto sandbox) and during production boot races, the targets table may not exist yet.

## Invariants

- `Canary.Repo` pool_size: 1. SQLite single-writer. All writes through this.
- `StateMachine.transition/4` is pure. No side effects. Table-driven tests.
- Summary generation is deterministic templates. No LLM on request path.
- RFC 9457 Problem Details for all error responses.
- No service names hardcoded. Targets/webhooks configured at runtime via API.
- Seeds only create a bootstrap API key. No hardcoded targets.

## Deploy

```bash
flyctl deploy --app canary-obs --remote-only
flyctl ssh console --app canary-obs -C "rm -f /data/canary.db*"  # nuclear reset
flyctl logs --app canary-obs --no-tail
```

Bootstrap API key logged on first boot — grep for `"Bootstrap API key:"`. Store it; it won't be shown again.

After deploy: re-register health targets and webhooks via API (they're in the DB that was just created fresh).

## canary-watch

Companion service at `misty-step/canary-watch`. Receives Canary webhooks, synthesizes GitHub issues via Gemini Flash structured output. Deployed at `canary-watch.fly.dev`.

Webhook secret from Canary registration must match `CANARY_WEBHOOK_SECRET` in canary-watch. Rotate both together.
