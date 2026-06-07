# Canary — Agent Router

Self-hosted observability substrate for AI agents (not humans). Rust + SQLite + Litestream → Fly Tigris. Fly app **`canary-obs`**. v1: single region, single org, one Docker image, one SQLite file. Read this before acting; read `CLAUDE.md` for load-bearing footguns.

## Stack & boundaries

| Layer | Owns | Path |
|---|---|---|
| Core service | HTTP surface, error ingest, health probing, correlation, timelines, queries, signed webhooks | `crates/canary-*` |
| TypeScript SDK | JS/TS client; `tsup` build + `vitest` | `clients/typescript/` |
| CI module | Single source of truth for the gate (Dagger TS) | `dagger/` |
| Bin scripts | Operator API — validate, dagger, bootstrap, DR | `bin/` |
| Backlog | File-driven work with `_done/` archive + priority map | `backlog.d/` |

External responders (e.g. bitterblossom) consume Canary's signed webhooks and query back. They live **outside this repo**.

## Ground-truth pointers (files that ARE the contract)

- **Agent-facing API contract:** `GET /api/v1/openapi.json` — source under `priv/openapi/`; `info.x-agent-guide` embeds the canonical replay guide.
- **Router + auth pipelines:** `crates/canary-server/src/lib.rs`, `crates/canary-server/src/server_auth.rs`, and route-family modules under `crates/canary-server/src/`.
- **Error-response shape (RFC 9457):** `crates/canary-http/src/problem_details.rs`.
- **Pure state machine:** `crates/canary-core/src/health/state_machine.rs` — transition logic has no side effects.
- **Webhook delivery ledger:** `crates/canary-server/src/webhook_delivery.rs` + `crates/canary-workers/src/webhooks.rs` (stable `X-Delivery-Id` across retries).
- **Health runtime:** `crates/canary-server/src/target_probes.rs`, `crates/canary-server/src/monitor_overdue.rs`, and `crates/canary-workers/src/health.rs`.
- **Alerter trio:** `crates/canary-server/src/webhooks.rs` and `crates/canary-workers/src/webhooks.rs` own signing, cooldown, and circuit decisions.
- **Query read models:** `crates/canary-store/src/query.rs` and `crates/canary-server/src/query_routes.rs`.
- **Ingest path:** `crates/canary-ingest/src/lib.rs`; Canary self-reporting uses Rust direct ingest, no HTTP loopback.
- **Schemas:** `crates/canary-store/src/schema.rs` — all custom string PKs keep stable prefixes (`ERR-`/`INC-`/`WHK-`/`MON-nanoid`).

Prefer these over re-deriving from the code base.

## Invariants (hard rules)

- **Single writer.** All writes go through `canary-store::Store`; the production runtime shares one writable SQLite store behind the server lock.
- **`StateMachine.transition/4` stays pure.** No side effects. Verified by table-driven tests.
- **Summaries are deterministic templates.** No LLM on the request path.
- **RFC 9457 Problem Details** for every error response.
- **Scoped API keys** (`ingest-only` / `read-only` / `admin`) enforced at the router. See `docs/api-key-rotation.md`.
- **Responder boundary.** Canary owns ingest/health/correlation/timelines/queries/webhooks. Repo mutation, issue creation, and LLM triage live downstream. Webhook payloads are stable product contracts.
- **No service names hardcoded.** Targets, monitors, and webhooks are configured at runtime via API. Seeds create only the bootstrap API key.
- **Target vs Monitor:** `Target` = HTTP URL probed on an interval. `Monitor` = check-in watcher for non-HTTP runtimes (desktop apps, cron, workers). Modes `schedule` or `ttl`. See `docs/non-http-health-semantics.md`.

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
- Rust workspace: format, check, clippy (`-D warnings`), tests.
- `clients/typescript/`: typecheck, coverage, build.
- Operator scripts: entrypoint, DR, and dogfood audit shell tests.
- Production image: Docker build + `/healthz` and `/readyz` smoke.

`bin/dagger` refuses CLI version drift from `dagger.json`. Do not hand-edit `.github/workflows/ci.yml` from a PR branch — the workflow lives outside the candidate diff per `docs/ci-control-plane.md`.

## Known-debt map

| Area | File(s) | Issue |
|---|---|---|
| **#010 Ramp pattern** (blocked, XL, north-star) | `backlog.d/010-ramp-pattern.md` | Blocked on bitterblossom triage sprite (`bitterblossom/backlog.d/011-canary-triage-sprite.md`). Agent-consumer shape of error→triage→fix. |
| **#020 Adminifi HTTP surface verification** (blocked, S) | `backlog.d/020-adminifi-http-surface-verification.md` | Upstream Adminifi HTTP surface stability. |
| Recurring footgun surface | `CLAUDE.md` footgun list + Rust store/runtime/schema modules | See `CLAUDE.md` — load-bearing. Every remediation here must cite the footgun list and extend it when new failure modes appear. |

All other tracked items are shipped and archived under `backlog.d/_done/`. Priority map + Lanes 1–5 in `backlog.d/README.md`.

## Outer loop

User-ratified composition: **`/settle → /refactor → /code-review → merge`.** Master keeps one squash commit per PR via `gh pr merge --squash`; PR title + body become that commit. Conventional-with-scope prefix on the PR title / squash subject (`feat(health):`, `fix(ci):`, `refactor(query):`, `chore(governance):`, `docs(ops):`, `build:`). Narrow test idiom: `cargo test -p <crate> <test_name> --locked`.

## Self-monitoring

Canary reports its own errors through the Rust runtime direct-ingest path, no HTTP loopback. Query Canary itself (`GET /api/v1/query?service=canary&window=5m`) for post-deploy signal.

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
