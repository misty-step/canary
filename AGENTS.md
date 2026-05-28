# Canary — Agent Router

Self-hosted observability substrate for AI agents (not humans). Phoenix/Elixir + SQLite + Litestream → Fly Tigris. Fly app **`canary-obs`**. v1: single region, single org, one Docker image, one SQLite file. Read this before acting; read `CLAUDE.md` for load-bearing footguns.

## Stack & boundaries

| Layer | Owns | Path |
|---|---|---|
| Core service | HTTP surface, error ingest, health probing, correlation, timelines, queries, signed webhooks | repo root (`lib/`, `test/`, `priv/`, `config/`) |
| Elixir SDK | `:logger` handler → async ingest; 90% coverage gate | `canary_sdk/` |
| TypeScript SDK | JS/TS client; `tsup` build + `vitest` | `clients/typescript/` |
| CI module | Single source of truth for the gate (Dagger TS) | `dagger/` |
| Bin scripts | Operator API — validate, dagger, bootstrap, DR | `bin/` |
| Backlog | File-driven work with `_done/` archive + priority map | `backlog.d/` |

External responders (e.g. bitterblossom) consume Canary's signed webhooks and query back. They live **outside this repo**.

## Ground-truth pointers (files that ARE the contract)

- **Agent-facing API contract:** `GET /api/v1/openapi.json` — source under `priv/openapi/`; `info.x-agent-guide` embeds the canonical replay guide.
- **Router + auth pipelines:** `lib/canary_web/router.ex` (pipelines `:scope_ingest | :scope_read | :scope_admin`).
- **Error-response shape (RFC 9457):** `lib/canary_web/problem_details.ex`.
- **Pure state machine:** `lib/canary/health/state_machine.ex` — `transition/4` has no side effects. Table-driven tests in `test/canary/health/state_machine_test.exs`.
- **Webhook delivery ledger:** `lib/canary/workers/webhook_delivery.ex` (stable `X-Delivery-Id` across retries).
- **Health supervisor:** `lib/canary/health/manager.ex` (the `rescue`-on-boot lives here).
- **Alerter trio:** `lib/canary/alerter/{circuit_breaker,cooldown,signer}.ex`.
- **Query read models (post-split):** `lib/canary/query.ex` + `lib/canary/query/{errors,health,incidents,search,window}.ex` (PR #125).
- **Ingest path:** `lib/canary/errors/ingest.ex`; `Canary.ErrorReporter` direct-ingest `:logger` handler (no HTTP loopback).
- **Schemas:** `lib/canary/schemas/*.ex` — all use custom string PKs (`ERR-`/`INC-`/`WHK-`/`MON-nanoid`).
- **Oban table migration:** `priv/repo/migrations/20260314230000_create_oban_jobs.exs` (never at runtime in a GenServer).

Prefer these over re-deriving from the code base.

## Invariants (hard rules)

- **Single writer.** `Canary.Repo` pool_size:1. All writes go through it. `Canary.ReadRepo` (pool_size:4) is **deliberately absent from `ecto_repos`** — only `Canary.Repo` runs migrations.
- **`StateMachine.transition/4` stays pure.** No side effects. Verified by table-driven tests.
- **Summaries are deterministic templates.** No LLM on the request path. Generators in `lib/canary/reports/*` and `lib/canary/*/summary.ex`.
- **RFC 9457 Problem Details** for every error response.
- **Scoped API keys** (`ingest-only` / `read-only` / `admin`) enforced at the router. See `docs/api-key-rotation.md`.
- **Responder boundary.** Canary owns ingest/health/correlation/timelines/queries/webhooks. Repo mutation, issue creation, and LLM triage live downstream. Webhook payloads are stable product contracts.
- **No service names hardcoded.** Targets, monitors, and webhooks are configured at runtime via API. Seeds create only the bootstrap API key.
- **Target vs Monitor:** `Target` = HTTP URL probed on an interval (`Canary.Health.Manager`). `Monitor` = check-in watcher for non-HTTP runtimes (desktop apps, cron, workers). Modes `schedule` or `ttl`. See `docs/non-http-health-semantics.md`.

## Gate contract

**`./bin/validate` IS the gate.** Do not invent parallel vocabulary.

| Invocation | Behavior | Wired to |
|---|---|---|
| `./bin/validate` | → `./bin/dagger check` (deterministic lanes + secrets scan) | manual run |
| `./bin/validate --fast` | → `dagger call fast` (lint + core tests) | `.githooks/pre-commit` |
| `./bin/validate --strict` | → `dagger call strict` (full gate + advisories + optional `.codex/agents/*.toml` validation when present) | `.githooks/pre-push` |
| `./bin/validate --advisories` | live advisory scan only | manual run |
| `dagger call strict --source=../candidate` | Hosted CI in `pull_request_target` immutable control plane (trusted base checkout at `.ci/trusted/`, candidate at `.ci/candidate/`) | `.github/workflows/ci.yml` |
| `flyctl deploy --app canary-obs --remote-only` | Auto on green master | `.github/workflows/deploy.yml` |

**Package gates inside strict:**
- Core: compile, format, credo (`--strict`), sobelow (medium), coverage **81%**, dialyzer.
- `canary_sdk/`: compile, format, coverage **90%**.
- `clients/typescript/`: typecheck, coverage, build.

`bin/dagger` refuses CLI version drift from `dagger.json`. Do not hand-edit `.github/workflows/ci.yml` from a PR branch — the workflow lives outside the candidate diff per `docs/ci-control-plane.md`.

## Known-debt map

| Area | File(s) | Issue |
|---|---|---|
| **#010 Ramp pattern** (blocked, XL, north-star) | `backlog.d/010-ramp-pattern.md` | Blocked on bitterblossom triage sprite (`bitterblossom/backlog.d/011-canary-triage-sprite.md`). Agent-consumer shape of error→triage→fix. |
| **#020 Adminifi HTTP surface verification** (blocked, S) | `backlog.d/020-adminifi-http-surface-verification.md` | Upstream Adminifi HTTP surface stability. |
| Recurring footgun surface | `CLAUDE.md` footgun list + `lib/canary/schemas/*`, `lib/canary/health/manager.ex`, `config/runtime.exs`, `priv/repo/migrations/20260314230000_*` | See `CLAUDE.md` — load-bearing. Every remediation here must cite the footgun list and extend it when new failure modes appear. |

All other tracked items are shipped and archived under `backlog.d/_done/`. Priority map + Lanes 1–5 in `backlog.d/README.md`.

## Outer loop

User-ratified composition: **`/settle → /refactor → /code-review → merge`.** Master keeps one squash commit per PR via `gh pr merge --squash`; PR title + body become that commit. Conventional-with-scope prefix on the PR title / squash subject (`feat(health):`, `fix(ci):`, `refactor(query):`, `chore(governance):`, `docs(ops):`, `build:`). Narrow test idiom: `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3`.

## Self-monitoring

Canary reports its own errors via `:logger` → `Canary.ErrorReporter` — direct ingest, no HTTP loopback. Query Canary itself (`GET /api/v1/query?service=canary&window=5m`) for post-deploy signal.

## Deploy (operational crib)

```bash
flyctl deploy --app canary-obs --remote-only       # happy path
flyctl storage create --app canary-obs --name canary-obs-backups --yes  # Tigris bootstrap
bin/dr-status                                       # read-only Litestream preflight
bin/dr-restore-check                                # non-destructive restore drill
```

Nuclear reset (human-gated, do NOT automate): stop machine → mount volume into maintenance machine → delete `/data/canary.db*` → destroy maintenance → restart real machine. Exact tested sequence in `docs/backup-restore-dr.md`.

Bootstrap API key logged once on first boot — grep `"Bootstrap API key:"` in Fly logs. Cannot be re-shown.

## Footguns

Load-bearing list lives in `CLAUDE.md`. Do not duplicate here — cite it.
