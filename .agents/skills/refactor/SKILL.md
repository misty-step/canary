---
name: refactor
description: |
  Branch-aware simplification and refactoring workflow. On feature branches,
  compare against base and simplify the diff before merge. On primary branch,
  scan the full codebase, research prior art, and identify the highest-impact
  simplification opportunity.
  Use when: "refactor this", "simplify this diff", "clean this up",
  "reduce complexity", "pay down tech debt", "make this easier to maintain",
  "make this more elegant", "reduce the number of states", "clarify naming".
  Trigger: /refactor.
argument-hint: "[--base <branch>] [--scope <path>] [--report-only] [--apply]"
---

# /refactor

Reduce complexity without reducing correctness in the canary-obs substrate.
Favor fewer states, clearer names, stronger invariants, behavior-preserving
tests, and current runbooks. Deletion first, then consolidation, then
abstraction, then mechanical cleanup. Ousterhout's lens is load-bearing:
prefer **deep modules** (simple interface, rich implementation) and delete
shallow pass-throughs.

## Canary invariants that bound every refactor

Refactors must not weaken these. If a proposed move touches any of them,
state explicitly how the invariant survives.

- `Canary.Repo` `pool_size: 1`. SQLite single-writer — all writes serialize
  through it. `ReadRepo` is deliberately absent from `ecto_repos`.
- `Canary.Health.StateMachine.transition/4` is pure. Table-driven tests.
  No side effects, no DB, no `DateTime.utc_now/0` inside. This file is the
  canonical deep-module exemplar: 121 LOC, one public function, rich state
  semantics behind a trivial signature.
- Summary generation is deterministic templates. No LLM on the request path,
  ever. A refactor that introduces a model call on ingest or webhook dispatch
  is a red-line violation.
- RFC 9457 Problem Details for every error response (`CanaryWeb.ProblemDetails`).
- OpenAPI stays in sync with router: `priv/openapi/*` must match
  `lib/canary_web/router.ex` pipelines (`:scope_ingest | :scope_read | :scope_admin`).
- Agent-native product shape: bounded + structured + summarized. Do not
  refactor toward dashboard-centric flows.

## Branch-Aware Routing

Detect the current branch and primary branch first:

1. Current: `git rev-parse --abbrev-ref HEAD`
2. Primary: `git symbolic-ref --short refs/remotes/origin/HEAD | sed 's#^origin/##'`
   (canary's primary is `master`)

If current branch != `master`: run **Feature Branch Mode**.
If current branch == `master`: run **Primary Branch Mode**.

If current branch resolves to `HEAD`, the primary branch cannot be discovered,
or the detected base is ambiguous, stop and require `--base <branch>`. Fail
closed rather than computing the wrong diff.

`--base <branch>` overrides detected base branch for feature-branch comparisons.
`--scope <path>` limits analysis and edits to one subtree (e.g.
`--scope lib/canary/query` or `--scope lib/canary/incidents.ex`).
`--report-only` disables file edits.
`--apply` allows edits in primary-branch mode (otherwise report + backlog
shaping only).

Detailed simplification methodology lives in `references/simplify.md`.

## Feature Branch Mode (default on PR branches)

Goal: simplify what changed between `origin/master...HEAD` before merge.

### 1. Map the delta

- Diff stats and touched files: `git diff --stat origin/master...HEAD`
- Touched file list: `git diff --name-only origin/master...HEAD`
- Identify high-leverage simplification targets:
  - pass-through layers (a facade that just forwards to one module — prefer
    `defdelegate` or inline the caller)
  - duplicate helpers across `lib/canary/query/*.ex`, `lib/canary/incidents.ex`,
    `lib/canary/incident_correlation.ex`
  - unclear naming, especially around incident lifecycle and correlation
  - unnecessary state branches in `lib/canary/health/state_machine.ex`
    (the transition table is the contract — mode flags elsewhere are often
    symptoms of leaking state)
  - over-modeled mode flags / booleans that should be a variant
  - tests that assert implementation (SQL fragments, exact error strings) —
    rewrite to assert behavior (returned shape, observable side effect)
  - stale docs in `docs/*.md` referenced by changed code paths

### 2. Parallel exploration bench

Launch at least three subagents in parallel:

- **Diff Cartographer (Explore):** map responsibilities and Ousterhout
  complexity smells across changed modules. Flag any new shallow wrapper.
- **Simplification Planner (Plan):** propose deletion/consolidation-first
  options. Each option names: files touched, LOC removed, invariants preserved.
- **Quality Auditor (Explore):** spot test/documentation gaps. For canary,
  that usually means: missing narrow test file, OpenAPI spec drift, or a
  changed webhook payload shape without a docs update.

Each subagent returns: top findings, one recommended change, confidence, risk.

### 3. Synthesize and choose

Rank opportunities by:

`(complexity removed * confidence) / implementation risk`

Prefer, in order:

1. deletion
2. consolidation
3. state-space reduction and invariant tightening
4. naming clarification
5. abstraction
6. mechanical refactor

### 4. Execute (unless `--report-only`)

Dispatch a builder subagent for exactly one bounded refactor. Each refactor
must include:

- behavior-preserving tests (new or updated) under `test/canary/<area>/`
- obvious naming improvements where needed
- doc updates for changed contracts — if the webhook payload shape or
  OpenAPI endpoint moved, update `priv/openapi/*` and the relevant
  runbook under `docs/`
- state reduction when the existing design encodes more modes than the
  observable behavior requires

### 5. Verify

Per-iteration loop (narrow, fast):

```bash
mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3
mix format
mix credo
./bin/validate --fast
```

Before push (full gate):

```bash
./bin/validate --strict
```

This runs `dagger call strict` — format, credo, sobelow, **coverage 81%**
(core), dialyzer, plus `canary_sdk/` gates (**coverage 90%**) and TS SDK
checks. Hosted CI re-validates via the immutable control plane
(`dagger call strict --source=../candidate`, see `docs/ci-control-plane.md`).

Exit criteria:

- narrow and strict gates green
- invariants preserved (explicitly noted in the PR body if the refactor
  touched any)
- complexity is **reduced, not moved** — if two modules now share the old
  cyclomatic weight, the refactor failed

## Primary Branch Mode (default on `master`)

Goal: find the single highest-impact simplification for canary core.
This mode is designed to be safe for scheduled runs.

### 1. Build a hotspot map

Use evidence from:

- churn: `git log --since='3 months' --name-only -- lib/ | sort | uniq -c | sort -rn | head -20`
- module size and fan-out on the canonical hot list:
  - `lib/canary/errors/ingest.ex`
  - `lib/canary/incidents.ex`
  - `lib/canary/query.ex` (already split in PR #125 — see prior art)
  - `lib/canary/timeline.ex`
  - `lib/canary/incident_correlation.ex`
  - `lib/canary/health/manager.ex`
  - `lib/canary/health/state_machine.ex`
  - `lib/canary/workers/webhook_delivery.ex`
  - `lib/canary/alerter/circuit_breaker.ex`,
    `lib/canary/alerter/cooldown.ex`,
    `lib/canary/alerter/signer.ex`
  - `lib/canary_web/router.ex`,
    `lib/canary_web/problem_details.ex`
- flaky or slow tests under `test/canary/`
- recurring failure domains from `backlog.d/` and `backlog.d/_done/`

### 2. Parallel strategy bench

Launch at least three subagents in parallel:

- **Topology Mapper (Explore):** locate architectural complexity hotspots
  in the hot list above. Specifically look for shallow-module patterns
  (thin facades, duplicated query helpers, state flags that encode the
  same distinction twice).
- **Deletion Hunter (Explore):** identify dead code, unused clauses in
  `Canary.Health.StateMachine`, compatibility shims with no active
  contract, and worker jobs no longer scheduled.
- **Rebuild Strategist (Plan):** propose the cleanest from-scratch shape
  for one hotspot. Reference prior art: PR #125
  (`refactor(query): split Canary.Query into domain read models`) —
  `lib/canary/query.ex` collapsed from a god-module into a 51-LOC facade
  over `lib/canary/query/{errors,health,incidents,search,window}.ex`,
  each a focused read model. That is the template: carve by domain, expose
  via `defdelegate`, keep observable behavior identical, update narrow
  tests module-by-module.

### 3. External calibration

Invoke `/research` for the target domain before final recommendation. Do
not assert architecture choices from memory. For canary, useful external
anchors are: Ecto/SQLite single-writer patterns, Oban Lite constraints,
Phoenix pipeline composition, observability substrate design (Sentry,
Honeycomb, OpenTelemetry collectors).

### 4. Produce outcome

Default (safe): no code edits. Instead:

- choose one winning candidate
- optionally list up to two runners-up as appendix only
- shape the winning opportunity into a concrete backlog item under
  `backlog.d/` using the next available ID (`#NNN-<slug>.md`), with a
  stated oracle (the observable signal that will tell us the refactor
  worked — e.g. "`lib/canary/incidents.ex` drops from 347 LOC to two
  files each under 200 LOC, same public API, same tests green")

If `--apply` is explicitly set:

- implement exactly one low-risk, bounded simplification for the winning
  candidate on a new branch
- verify with `./bin/validate --strict`
- record residual risk and follow-up items

### Commit and landing conventions

- Commit scope matches the hot module: `refactor(query):`, `refactor(health):`,
  `refactor(webhooks):`, `refactor(alerter):`, `refactor(incidents):`,
  `refactor(ingest):`, `refactor(timeline):`, `refactor(correlation):`.
- Linear history. No squash on `master`. If the refactor is a sequence of
  extractions (as in PR #125: three commits, one per extracted read model),
  preserve each as a standalone commit.
- Gate: `./bin/validate --strict` before push. Hosted CI re-runs the same
  Dagger entrypoint; local and CI share the control plane by design.

## Required Output

```markdown
## Refactor Report
Mode: feature-branch | primary-branch
Target: <branch or scope>

### Candidate Opportunities
1. [winning candidate] — complexity removed, risk, confidence

### Optional Runners-Up
1. [runner-up]
2. [runner-up]

### Selected Action
[what was applied, or backlog item created under backlog.d/]

### Invariants Preserved
[explicit note: pool_size:1, StateMachine purity, deterministic summaries,
RFC 9457 shape, OpenAPI/router parity — which ones the refactor touched and
how it preserved them]

### Verification
[./bin/validate --fast / --strict results, narrow test file runs]

### Residual Risks
[what remains and why]
```

## Gotchas

- **Complexity moved, not removed:** splitting `lib/canary/incidents.ex` into
  two 200-LOC modules that share state is not simplification. PR #125 worked
  because each read model owns a disjoint query surface.
- **Shallow extraction:** creating a new module that exposes one function
  called from exactly one place is almost always wrong. Inline it or
  `defdelegate` from the existing facade.
- **"Refactor everything":** broad edits destroy reviewability. Keep each
  pass bounded to one commit scope (`refactor(<scope>):`).
- **Skipping branch mode detection:** primary and feature branches have
  different risk envelopes. `master` defaults to report + shaping.
- **Applying risky changes on primary by default:** primary mode requires
  `--apply` for edits.
- **No oracle for a proposed refactor:** if you cannot state how success
  is measured (LOC, fan-out, removed states, coverage delta), the proposal
  is not ready.
- **Chasing aesthetic churn:** clearer names and fewer states matter;
  style-only motion does not. `mix format` already runs in `./bin/validate --fast`.
- **Parallelizing dependent edits:** only parallelize disjoint slices.
  Extracting `Canary.Query.Errors` and `Canary.Query.Health` can run in
  parallel; touching `lib/canary_web/router.ex` and its OpenAPI spec
  cannot.
- **Bypassing narrow tests:** `mix test` alone is slow and noisy. Always
  iterate with `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3`
  until green, then escalate to `./bin/validate --strict`.
- **Touching invariant-bearing modules without re-reading the contract:**
  before editing `lib/canary/health/state_machine.ex`, re-read its
  docstring and table-driven tests. It is the deep-module exemplar for a
  reason.
