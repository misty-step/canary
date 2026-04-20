---
name: canary repo brief
description: Shared spine for tailored harness primitives — stack, gate, invariants, debts, terminology, session signal
last-updated: 2026-04-20
---

# Canary — Repo Brief

Shared spine for every tailored primitive in `.agents/skills/`. Cite these
anchors verbatim. Do not invent parallel vocabulary.

## Vision & purpose

Canary is an **agent-first** observability substrate — error ingestion plus
health/check-in monitoring — **for AI agents, not human operators**. Agents
are the primary consumer of `summary` fields, OpenAPI `info.x-agent-guide`,
signed generic webhooks, and scoped API keys. Dashboards exist as a
fallback (`/dashboard` LiveView, password-gated) but are not the product
surface. Vision doctrine lives in `VISION.md`; operating principles (agent-
first, single deployable, broadcast-don't-prescribe, deterministic over
probabilistic) in `PRINCIPLES.md`.

## Stack & boundaries

Single Elixir/OTP application, one Docker image, one SQLite file + WAL,
one Fly.io app.

| Layer | Owns | Path |
|---|---|---|
| Core service | HTTP surface, error ingest, health probing, correlation, timelines, queries, signed webhooks | repo root (`lib/`, `test/`, `priv/`, `config/`) |
| Elixir SDK | `:logger` handler → async ingest; 90% coverage gate | `canary_sdk/` |
| TypeScript SDK | JS/TS client; `tsup` build + `vitest` | `clients/typescript/` |
| CI module | Single source of truth for the gate (Dagger TS) | `dagger/` |
| Bin scripts | Operator API — validate, dagger, bootstrap, DR | `bin/` |
| Backlog | File-driven work with `_done/` archive + priority map | `backlog.d/` |
| Harness | Shared skill root + per-harness bridges; Claude Code + Codex | `.agents/`, `.claude/`, `.codex/` |

**Responder boundary.** Canary owns ingest / health / correlation / timelines
/ queries / webhooks. **Repo mutation, issue creation, and LLM triage live
downstream** in separate consumers (e.g. bitterblossom). Webhook payloads
are stable product contracts, not app-specific glue.

**Elixir/Phoenix 1.8 + Bandit + Ecto SQLite3 + Oban (SQLite engine) + Req +
Finch + Litestream → Fly Tigris.** Dagger TS (engine `v0.20.5`) owns the
pipeline. Deploy to Fly app `canary-obs` (region `iad`).

## Load-bearing gate

**`./bin/validate` IS the gate.** Cite it verbatim. Do not invent parallel
vocabulary.

| Invocation | Delegates to | Wired into |
|---|---|---|
| `./bin/validate` | `./bin/dagger check` | manual |
| `./bin/validate --fast` | `dagger call fast` | `.githooks/pre-commit` |
| `./bin/validate --strict` | `dagger call strict` | `.githooks/pre-push` |
| `./bin/validate --advisories` | `dagger call advisories` | manual CVE triage |
| `dagger call strict --source=../candidate` | immutable control plane (trusted base at `.ci/trusted/`, candidate at `.ci/candidate/`) | `.github/workflows/ci.yml` |
| `flyctl deploy --app canary-obs --remote-only` | hosted CI green on `master` | `.github/workflows/deploy.yml` |

**Package gates inside strict:**
- Core: compile, format, credo `--strict`, sobelow (medium), coverage
  **81%**, dialyzer.
- `canary_sdk/`: compile, format, coverage **90%**.
- `clients/typescript/`: typecheck, coverage, build.

`bin/dagger` refuses local CLI drift from `dagger.json`. Hosted CI uses
`pull_request_target` and cannot be weakened from a PR branch — the
workflow + `dagger/` module live outside the candidate diff. Authoritative
runbook: `docs/ci-control-plane.md`.

## Invariants

- **Single writer.** `Canary.Repo` `pool_size: 1`. All writes through it.
  `Canary.ReadRepo` (`pool_size: 4`) is **deliberately absent from
  `ecto_repos`** — only `Canary.Repo` runs migrations.
- **`StateMachine.transition/4` is pure.** No side effects. Table-driven
  tests in `test/canary/health/state_machine_test.exs`.
- **Summaries are deterministic templates.** No LLM on the request path.
  Generators under `lib/canary/reports/*` and `lib/canary/*/summary.ex`.
- **RFC 9457 Problem Details** for every error response
  (`lib/canary_web/problem_details.ex`).
- **Scoped API keys** (`ingest-only` / `read-only` / `admin`) enforced at
  the router via `:scope_ingest | :scope_read | :scope_admin` pipelines
  (`lib/canary_web/router.ex`). Rotation guide: `docs/api-key-rotation.md`.
- **Responder boundary** (see above).
- **No service names hardcoded.** Targets, monitors, webhooks are runtime-
  configured via API. Seeds create only the bootstrap API key.
- **Target vs Monitor.** `Target` = HTTP URL probed on an interval
  (`Canary.Health.Manager`). `Monitor` = check-in watcher for non-HTTP
  runtimes (desktop apps, cron, workers). Modes `schedule` or `ttl`. See
  `docs/non-http-health-semantics.md`.
- **Linear history on master.** `git merge --ff-only` or
  `gh pr merge --merge` (never `--squash`). Multi-commit branches must
  pass the gate per-commit via
  `git rebase -x './bin/validate --strict' origin/master`.
- **Conventional commits with scope.** From `git log`: `feat(health):`,
  `fix(ci):`, `refactor(query):`, `chore(governance):`, `docs(ops):`,
  `build:`, `chore(backlog):`.

## Known debts

| Area | File(s) | Issue |
|---|---|---|
| **#010 Ramp pattern** (blocked, XL, north-star) | `backlog.d/010-ramp-pattern.md` | Agent-consumer shape of error→triage→fix. Blocked on bitterblossom triage sprite (`bitterblossom/backlog.d/011-canary-triage-sprite.md`). |
| **#020 Adminifi HTTP surface verification** (blocked, S) | `backlog.d/020-adminifi-http-surface-verification.md` | Upstream Adminifi HTTP surface stability. |
| **Footguns (load-bearing)** | `CLAUDE.md` footgun list; `lib/canary/schemas/*`, `lib/canary/health/manager.ex`, `config/runtime.exs`, `priv/repo/migrations/20260314230000_*` | Ecto custom PK cast-drop; Oban Lite table migration via Ecto (not GenServer); Req/Finch `:finch` + `:connect_options` conflict; `ReadRepo` must NOT be in `ecto_repos`; `runtime.exs` `http: [port:]` key; `Health.Manager` `rescue`-on-boot; SQLite WAL survives `rm -f`; circuit-breaker ETS reset = restart canary-obs. |

All other tracked items shipped and archived under `backlog.d/_done/`
(21 items as of 2026-04-20). Priority map + Lanes 1–5 in
`backlog.d/README.md`.

## Terminology

- **Target** = HTTP probe subject (GenServer per target via
  `Canary.Health.Manager`). Not a "monitor" in Canary vocabulary.
- **Monitor** = non-HTTP check-in watcher (Oban-scheduled). Product
  concept, distinct from `/monitor` the harness skill.
- **Ingest** = `Canary.Errors.Ingest.ingest/1`. Validate → group_hash →
  persist → webhook.
- **Ramp pattern** = error→auto-triage→fix loop; Canary's north-star UX
  (`backlog.d/010-ramp-pattern.md`).
- **Triage sprite** = bitterblossom-side agent consumer that closes the
  Ramp loop. Not in this repo.
- **Gate** = `./bin/validate` and only `./bin/validate`. "CI" in prose
  refers to the hosted GitHub workflow, which is the same gate.
- **Dogfood audit** = `./bin/dogfood-audit [--strict]` — canary watching
  its own networked services.

## Session signal

**Recurring user corrections / validated patterns** (from session
history + memory):

1. **Every product is demoable.** Don't skip `/demo` on "no marketing
   surface" / "operator-only dashboard" grounds. API + SDK + webhook
   replay counts. (Memory: `feedback_demoable_scope`.)
2. **Distinguish skills by operational shape, not name.** `/yeet` (worktree
   → push) vs `/settle` (branch landing) are distinct, even though both
   "ship." Same for `/demo` vs `/qa`.
3. **Custom string PKs must be set on the struct, not cast.** Ecto
   silently drops `id` fields that aren't in `@required`/`@optional`.
   Six bugs in initial build. (Memory: `feedback_ecto_custom_pks`.)
4. **Oban Lite does not auto-create its tables.** Use a dedicated Ecto
   migration with `execute`, never a GenServer + `Repo.query!` (races
   with `pool_size: 1`). (Memory: `feedback_oban_sqlite`.)
5. **Project skills belong in the shared repo skill root**, not global
   `~/.claude/skills/`. (Memory: `feedback_skill_placement`.)

**Validated approaches** (user-ratified, keep using):

- Router-style `AGENTS.md` with stack/boundaries, ground-truth pointers,
  invariants, gate contract, known-debt map, harness index tables. Not
  a prose codex.
- Prior tailor output (c2c8e35) — single gate citation of
  `./bin/validate`, coverage names `81% core / 90% canary_sdk`,
  conventional commit scopes taken from `git log`, linear-no-squash,
  responder boundary enforced in acceptance criteria. All validated.
- Load-bearing footgun list stays in `CLAUDE.md`. Never duplicate into
  `AGENTS.md` or skill bodies — cite it.
