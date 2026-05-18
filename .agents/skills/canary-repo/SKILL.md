---
name: canary-repo
description: |
  Work safely inside the Canary repository: Phoenix/Elixir core service,
  Elixir SDK, TypeScript SDK, Dagger CI, OpenAPI/router contracts, ingest,
  health, monitors, webhooks, query read models, alerter, backlog, and harness
  changes. Use before editing Canary product code or repo-local harness files.
  Triggers: Canary repo, canary codebase, implement in Canary, fix Canary,
  modify lib/canary, OpenAPI parity, StateMachine, scoped API keys.
argument-hint: "[area, e.g. health, ingest, query, webhooks, sdk, ci, harness]"
---

# /canary-repo

Use this skill when changing this repository. Use `/canary` instead when the
task is only querying or operating a running Canary instance through its API.

## Read First

- `AGENTS.md` for the repo map, gate contract, backlog lanes, harness index,
  and agent/persona routing.
- `CLAUDE.md` for load-bearing footguns and invariants. Preserve those
  sections verbatim unless appending a newly proven footgun.
- `README.md` for current setup, package layout, and validation commands.
- The narrow module files named by `AGENTS.md` before touching that subsystem.

## Stack Map

- Core service: Phoenix/Elixir at repo root (`lib/`, `test/`, `priv/`,
  `config/`).
- Elixir SDK: `canary_sdk/`, with its own 90% coverage gate.
- TypeScript SDK: `clients/typescript/`, built with `tsup` and `vitest`.
- CI control plane: `dagger/`, invoked through `./bin/dagger` and
  `./bin/validate`.
- Backlog: `backlog.d/`, with completed work archived under
  `backlog.d/_done/`.
- Harness source: `.agents/skills/`; `.claude/skills/` and `.codex/skills/`
  are symlink bridges.

## Non-Negotiable Invariants

- `./bin/validate` is the gate vocabulary. `--fast`, default/check,
  `--strict`, and `--advisories` map to Dagger lanes; do not invent parallel
  test vocabulary.
- SQLite single-writer: `Canary.Repo` has `pool_size: 1`; all writes go
  through it. `Canary.ReadRepo` stays out of `ecto_repos`.
- Custom string PKs are set on structs before changesets, not cast through
  attrs. The `ERR-`/`INC-`/`WHK-`/`MON-` IDs are product contracts.
- `Canary.Health.StateMachine.transition/4` is pure. Side effects belong in
  managers/workers around it, never inside it.
- Error responses use RFC 9457 Problem Details through
  `CanaryWeb.ProblemDetails`.
- Summaries are deterministic templates. No LLM on ingest, query, webhook, or
  request paths.
- Router scope pipelines enforce `ingest-only`, `read-only`, and `admin` API
  keys. OpenAPI must match router behavior.
- Canary owns ingest, health/check-ins, correlation, timelines, queries, and
  signed generic webhooks. Repo mutation, issue creation, and LLM triage belong
  downstream.
- Runtime targets, monitors, and webhooks are configured by API; do not
  hardcode service names.
- Oban SQLite tables are created by the dedicated Ecto migration, not by a
  GenServer or release-time query.

## Routing By Area

- HTTP/auth/API shape: read `lib/canary_web/router.ex`,
  `lib/canary_web/problem_details.ex`, and `priv/openapi/`; consider
  `api-design-specialist` and `security-sentinel`.
- Ingest and direct logger reporter: read `lib/canary/errors/ingest.ex` and
  `Canary.ErrorReporter`; consider `carmack`.
- Health targets and state: read `lib/canary/health/manager.ex`,
  `lib/canary/health/state_machine.ex`, and
  `test/canary/health/state_machine_test.exs`; consider `ousterhout` or
  `beck`.
- Query/read models: read `lib/canary/query.ex` and
  `lib/canary/query/{errors,health,incidents,search,window}.ex`; keep bounded
  fetches in the database.
- Webhooks/alerter: read `lib/canary/workers/webhook_delivery.ex` and
  `lib/canary/alerter/{circuit_breaker,cooldown,signer}.ex`.
- Schemas/migrations: read `lib/canary/schemas/*.ex` and relevant
  `priv/repo/migrations/*`; consider `data-integrity-guardian`.
- CI/gates: use `/ci`; the source of truth is `dagger/src/index.ts` plus
  `./bin/validate`.
- Harness changes: use `/harness`; edit `.agents/skills/` canonical copies,
  then keep `.claude/skills/` and `.codex/skills/` symlinks in parity.

## Verification Ladder

Use the smallest useful check first, then escalate with risk:

```sh
mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3
mix format
./bin/validate --fast
./bin/validate
./bin/validate --strict
```

Package-specific checks:

- Core coverage gate: 81%.
- `canary_sdk/` coverage gate: 90%.
- TypeScript SDK: typecheck, coverage, and build through the Dagger lanes.
- Codex agent roles: `./bin/dagger call codex-agent-roles`.

## Change Discipline

- Keep work scoped to the subsystem and backlog item in front of you.
- For public contract changes, update tests, OpenAPI, docs, and SDKs together
  or explicitly explain why a surface is unaffected.
- For webhook payload changes, treat shape stability as a product contract.
- For production ops changes, preserve the Fly app `canary-obs`, `/healthz`,
  `/readyz`, Litestream/Tigris backup assumptions, and the human-gated nuclear
  reset procedure in `docs/backup-restore-dr.md`.
- Do not edit `.claude/skills/` or `.codex/skills/` content directly; they are
  bridges.
- Do not make dashboards the primary UI. Canary is agent-first; API responses,
  summaries, and replayability matter more than screens.
