---
name: agent-readiness
description: |
  Assess and improve codebase readiness for AI coding agents. Dispatches
  parallel subagents to evaluate style, testing, docs, architecture, CI,
  observability, security, and dev environment. Produces a scored report
  with prioritized remediation. Then executes the highest-impact fixes.
  Use when: "agent readiness", "is this codebase agent-ready",
  "readiness report", "make this codebase agent-friendly",
  "agent-ready assessment", "readiness audit", "prepare for agents".
  Trigger: /agent-readiness, /readiness.
argument-hint: "[--assess-only] [--fix] [--pillar <name>] [--product]"
---

# /agent-readiness (canary)

In this repo, "agent-readiness" is a **dual audit**. Canary is simultaneously
a codebase agents work *in* and a product whose primary UI is agents — so
every run scores two surfaces and fixes the one with the widest gap.

**Target:** $ARGUMENTS

## The Two Audiences

**1. Codebase readiness (internal).** Can an AI coding agent land a change
in this monorepo confidently — format, lint, test, validate, deploy — without
being hand-held? Standard pillars, canary-specific evidence.

**2. Product readiness (external).** Is Canary's *own API* a good target
for downstream agents (responders like `bitterblossom`)? Canary's vision
(`VISION.md`) is explicit: "agents are the UI." The product is a substrate
for agent-driven infrastructure. A codebase that passes L5 internally but
ships an API agents can't reason over has failed the north star.

A `/agent-readiness` run that treats this as purely a codebase audit and
ignores the OpenAPI `info.x-agent-guide`, the deterministic `summary`
fields, the scoped keys, or the responder boundary is incomplete by
construction. Score both.

## Execution Stance

You are the executive orchestrator.
- Keep prioritization, remediation approval, and final readiness judgment on the lead model.
- Delegate pillar assessments and bounded fixes to focused Explore/builder subagents.
- Parallel fanout by default — pillars are independent.
- Remediate directly when the work is mechanical or <30 LOC single-concern; do not hand off a report when action would suffice.

## Canonical Commands, Paths, and Invariants

These are load-bearing. Every subagent cites them verbatim; the skill never
invents parallel vocabulary.

**The gate.** `./bin/validate` (default → `./bin/dagger check`), `--fast`
(pre-commit), `--strict` (pre-push + hosted CI), `--advisories` (live scan).
Hosted CI mirrors `--strict` through the immutable control plane in
`docs/ci-control-plane.md` (`pull_request_target` with trusted base +
candidate checkouts). `./bin/dagger` refuses local CLI version drift.

**Coverage thresholds.** Core **81%**, `canary_sdk/` **90%**. Non-negotiable —
the red line "never lower quality gates" applies here. A readiness
recommendation that proposes lowering either threshold is an invariant
violation, not a fix.

**Hot modules.** `lib/canary/errors/ingest.ex`, `lib/canary/incidents.ex`,
`lib/canary/query.ex` (deep-module split, PR #125),
`lib/canary/incident_correlation.ex`, `lib/canary/timeline.ex`,
`lib/canary/health/{manager,state_machine}.ex`,
`lib/canary/webhooks/delivery.ex`,
`lib/canary/alerter/{circuit_breaker,cooldown,signer}.ex`,
`lib/canary_web/router.ex`, `lib/canary_web/plugs/problem_details.ex`.

**Agent-facing product contract.**
- `GET /api/v1/openapi.json` is the canonical machine-readable spec; it
  embeds the replay guide in `info.x-agent-guide` (webhook → timeline
  cursor → annotations flow). Router changes MUST update `priv/openapi/openapi.json`.
- Every read response includes a deterministic NL `summary` field produced
  by `Canary.Summary` (pure templates, no LLM on request path).
- All error responses use RFC 9457 Problem Details via
  `CanaryWeb.Plugs.ProblemDetails` (`application/problem+json`).
- Scoped API keys enforced by router pipelines `:scope_ingest`,
  `:scope_read`, `:scope_admin`.
- Webhooks are HMAC-SHA256 signed; `X-Delivery-Id` is stable across retries;
  payloads are stable product contracts.
- **Responder boundary.** Canary owns ingest/health/correlation/timelines/
  queries/webhooks. Repo mutation, issue creation, and LLM triage live in
  downstream responders. A readiness recommendation that proposes adding
  any of those to the core service is an invariant violation — reject it.

**Invariants that constrain remediation.**
- `Canary.Repo` pool_size:1 (single SQLite writer).
- `Canary.Health.StateMachine.transition/4` is pure, table-driven.
- Summaries are deterministic templates (no LLM on request path).
- OpenAPI is the contract — any router-shape change updates `priv/openapi/*`.
- Responder boundary — do not pull downstream behavior upstream.

## Workflow

### Phase 1: Parallel Assess — Codebase Pillars

Dispatch one Explore subagent per pillar simultaneously. Each reads
`references/pillar-checks.md` for its specific pass/fail criteria and
grounds its evidence in the canary paths below.

| # | Pillar | Canary evidence to inspect |
|---|--------|----------------------------|
| 1 | Style & Validation | `.formatter.exs`, `mix.exs` credo + sobelow deps, `.credo.exs`, `./bin/validate --fast`, `.githooks/pre-commit` |
| 2 | Build & CI | `./bin/validate`, `./bin/dagger`, `dagger/src/index.ts`, `dagger.json` (pinned CLI), `.github/workflows/ci.yml`, `.github/workflows/deploy.yml`, `docs/ci-control-plane.md` |
| 3 | Testing | `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3` ergonomics, core **81%** / SDK **90%** gates, `test/support/`, Ecto sandbox in `config/test.exs` |
| 4 | Documentation | `CLAUDE.md` (footguns), `AGENTS.md` (invariants if present), `README.md`, `VISION.md`, `PRINCIPLES.md` (if present), `docs/*.md` runbooks, `backlog.d/README.md` |
| 5 | Dev Environment | `./bin/bootstrap`, `.tool-versions` (Erlang 27.3.4.9, Elixir 1.17.3-otp-27, Node 22.22.0), `.env.example`, `bin/dagger` version gate, `dagger.json` pin |
| 6 | Code Quality | Deep-module boundaries (see `lib/canary/query.ex` split PR #125), dialyzer gate in strict, shallow pass-throughs, dead code |
| 7 | Observability | `Canary.ErrorReporter` (self-monitors via `:logger` handler, direct-ingest no-HTTP), `lib/canary_web/telemetry.ex`, `lib/canary/json_logger.ex`, `GET /healthz`, `GET /readyz`, `GET /metrics` |
| 8 | Security & Governance | Scoped keys + `CanaryWeb.Plugs.RequireScope`, sobelow gate, `mix_audit` + gitleaks via Dagger, `docs/api-key-rotation.md`, `docs/governance.md`, `.codex/agents/*.toml` role validation |

Each subagent returns:

```markdown
## [Pillar]
Score: X/Y checks passed
Maturity: L1-L5

### Passing
- [check]: [evidence — cite file path + line or command output]

### Failing
- [check]: [what's missing] -> [fix, concrete and bounded]

### Highest-Impact Fix
[The single change that would most move this pillar]
```

### Phase 1b: Product Readiness — Canary's Own API

Always run this pillar, even on `--pillar <name>`, unless
`--no-product` is explicitly passed. This is what separates the canary
variant from a generic readiness audit.

Dispatch one Explore subagent with this checklist:

- [ ] `GET /api/v1/openapi.json` reachable and current; schema matches
      live router surface in `lib/canary_web/router.ex`.
- [ ] `info.x-agent-guide` present with `summary`, `flow`, `webhook_contract`,
      `crash_recovery`, `cursor_precedence`. Cite `priv/openapi/openapi.json`.
- [ ] Every read endpoint response includes a deterministic NL `summary`
      field produced by `Canary.Summary` (no LLM on request path).
      Evidence: `lib/canary/summary.ex` + controller render sites.
- [ ] All error paths emit RFC 9457 Problem Details
      (`application/problem+json`) via `CanaryWeb.Plugs.ProblemDetails`.
- [ ] Scoped keys enforced at the pipeline layer (`:scope_ingest`,
      `:scope_read`, `:scope_admin`) — not per-controller.
- [ ] Webhook payload shape, `X-Delivery-Id` stability across retries,
      and HMAC-SHA256 signing are documented as stable product contracts
      (`README.md` webhook section + `lib/canary/webhooks/delivery.ex` +
      `lib/canary/alerter/signer.ex`).
- [ ] Responder boundary is documented (`CLAUDE.md` Responder Boundary
      section) and honored — no repo mutation, issue creation, or LLM
      triage in core.
- [ ] Timeline is positioned as the durable source of truth; webhooks are
      wake-up hints. Evidence: `info.x-agent-guide.webhook_contract` +
      `GET /api/v1/timeline` cursor semantics.
- [ ] Backlog item status for agent-contract work in `backlog.d/` and
      `backlog.d/_done/` (e.g. `#011`, `#012`, `#013` archived; active
      blockers named).

Output mirrors the pillar format.

### Phase 2: Synthesize

Collapse all nine pillars (8 codebase + 1 product) into one report:

```markdown
# Agent Readiness Report: canary

## Overall: Level X — [name] (XX%)

| Pillar | Score | Level | Top Fix |
|--------|-------|-------|---------|
| Style & Validation | N/M | Lk | ... |
| Build & CI | N/M | Lk | ... |
| Testing | N/M | Lk | ... |
| Documentation | N/M | Lk | ... |
| Dev Environment | N/M | Lk | ... |
| Code Quality | N/M | Lk | ... |
| Observability | N/M | Lk | ... |
| Security & Governance | N/M | Lk | ... |
| Product API (agent-facing) | N/M | Lk | ... |

## Baseline (expected prior — verify, don't assume)
- CI: strong (`./bin/validate --strict` load-bearing, immutable control plane, Dagger pinned).
- Docs: strong (`CLAUDE.md` + `docs/*.md` runbooks + `VISION.md`).
- Testing: strong (core **81%** / `canary_sdk` **90%** gates enforced in strict).
- Observability: unique — repo IS an observability substrate and self-monitors via `Canary.ErrorReporter`.
- Architecture: good (deep modules; `lib/canary/query.ex` split PR #125).
- Style: strong (credo + sobelow + mix_audit in the gate).
- Security: strong (scoped keys + sobelow + gitleaks via Dagger).
- Dev env: good (`./bin/bootstrap`; `.tool-versions` pinned; `bin/dagger` refuses drift).
- Product API: the differentiator — score it honestly.

## Top 5 Recommendations (by impact)
[ordered across all pillars — product-API gaps often outrank codebase gaps in canary]

## Detailed Findings
[per-pillar blocks]
```

Maturity gate: must pass 80% at the current level and all prior levels
before advancing. No cherry-picking.

### Phase 3: Clarify

Present the report. Ask one question at a time:
1. Which recommendations should we execute now?
2. Any pillars to skip or deprioritize?
3. Any constraints (stay within existing tooling, skip OpenAPI churn, etc.)?

### Phase 4: Fix

For each approved recommendation, act at the appropriate level of
delegation (executive protocol). Mechanical? Run it. Design judgment?
Dispatch a builder subagent. Parallel-safe? Fan out to worktrees.

Canary-specific fix patterns:

| Fix type | Example | Approach |
|----------|---------|----------|
| Style/hook tightening | Add a credo check, tighten `.formatter.exs` | Direct edit; verify with `./bin/validate --fast` |
| Coverage backfill | Raise a shallow module above the **81%** floor | Builder + TDD; run `mix test ... --trace --max-failures 3` narrow |
| Docs footgun capture | A bug you just hit -> `CLAUDE.md` Footguns section | Direct edit; commit `docs(ops): codify <footgun>` |
| Deep-module refactor | Collapse a pass-through or split like `lib/canary/query.ex` | Builder subagent; update callers; extend invariant tests |
| CI enhancement | New check inside `dagger call strict` | Edit `dagger/src/index.ts`; regenerate source ignore lists via `python3 dagger/scripts/sync_source_arguments.py --write`; verify `./bin/validate --strict` |
| Product: missing `summary` | Controller returns payload without NL summary | Add `Canary.Summary.<fn>/1`; wire into controller; snapshot-test the template |
| Product: non-Problem-Details error | 4xx/5xx renders raw JSON | Route through `CanaryWeb.Plugs.ProblemDetails.render_error/5` |
| Product: OpenAPI drift | Router change without spec update | Update `priv/openapi/openapi.json` in same commit |
| Product: agent-guide gap | `info.x-agent-guide` missing a flow step | Update `priv/openapi/openapi.json`; add regression test that asserts keys present |
| Self-observability gap | A failure mode Canary can't see in itself | Wire via `Canary.ErrorReporter` (direct-ingest, never HTTP to self) |
| Security hardening | New endpoint not behind a scope pipeline | Add to correct `scope "/"` block in `lib/canary_web/router.ex`; add negative test |

Commit style is conventional-with-scope:

- `feat(readiness): add /api/v1/openapi.json regression test for x-agent-guide keys`
- `docs(ops): codify <footgun> in CLAUDE.md Footguns`
- `refactor(query): collapse pass-through in <module>`
- `chore(governance): tighten <rule> in .credo.exs`
- `fix(ci): ...`

After each fix: re-run the narrowest gate that exercises it, then
`./bin/validate --strict` before the next.

### Phase 5: Re-assess (optional)

On `--fix`, re-run Phases 1 + 1b and show before/after deltas per pillar.
Call out any pillar that moved a level.

## Routing

| Argument | Behavior |
|----------|----------|
| (none) | Full assess -> report -> clarify -> fix |
| `--assess-only` | Phases 1 + 1b + 2 only |
| `--fix` | Skip clarification; execute top 5 |
| `--pillar <name>` | Single pillar; still includes product pillar |
| `--product` | Only the product-API pillar (Phase 1b) |

## Gotchas (canary)

- **Never lower `./bin/validate` gates.** Core **81%**, `canary_sdk` **90%**,
  credo, sobelow, dialyzer, mix_audit, gitleaks. Red line.
- **Do not route around the gate.** If strict is red, fix the cause. Skipping
  hooks (`--no-verify`) or bypassing `./bin/dagger` is never the remediation.
- **OpenAPI is the contract.** A router change without a matching
  `priv/openapi/openapi.json` update is a broken agent-facing product,
  regardless of test coverage.
- **No LLM on the request path, ever.** If a pillar finding proposes
  LLM-generated summaries or on-request triage, it violates an invariant —
  reject and file as a downstream-responder concern.
- **Responder boundary is load-bearing.** Do not recommend pulling issue
  creation, repo mutation, or LLM triage into the core service. Those live
  in downstream consumers that subscribe to webhooks + query Canary for
  context.
- **Single writer.** `Canary.Repo` pool_size:1. Recommendations that assume
  concurrent writers are wrong for SQLite and wrong for this repo.
- **Self-monitoring is direct ingest, not HTTP.** `Canary.ErrorReporter`
  calls `Canary.Errors.Ingest.ingest/1` in-process. Do not "improve" it
  into an HTTP loopback — that creates a failure mode where Canary can't
  report its own errors when its HTTP layer is the thing that's broken.
- **`ReadRepo` is deliberately not in `ecto_repos`.** Only `Canary.Repo`
  runs migrations. A fix that adds `ReadRepo` to `ecto_repos` breaks
  `mix ecto.migrate`.
- **Ecto custom string PKs** (`ERR-*`, `INC-*`, `WHK-*`) must be set on the
  struct, not cast. See `CLAUDE.md` Footguns. A "readiness" fix that normalizes
  changesets without this carve-out silently drops IDs.
- **CLAUDE.md > README for agents.** If agents ignore a doc, it doesn't
  matter how pretty it is. Codify corrections into `CLAUDE.md` Footguns or
  Invariants — not README prose.
- **Scoring without fixing is theater.** Always push to Phase 4. Boil the
  ocean — ship the finished fix, not a plan to build the fix.
- **Monorepo score is the floor.** Core, `canary_sdk/`, `clients/typescript/`,
  `dagger/` — the weakest package caps the overall rating, not the average.

## Pillar Check Reference

`references/pillar-checks.md` defines binary pass/fail criteria per pillar.
`references/agent-readiness-principles.md` is the deeper "why." Both are
framework-neutral; canary-specific evidence lives in the tables above.
