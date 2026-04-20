---
name: shape
description: |
  Shape a raw idea into something buildable. Product + technical exploration.
  Spec, design, critique, plan. Output is a context packet.
  Use when: "shape this", "write a spec", "design this feature",
  "plan this", "spec out", "context packet", "technical design".
  Trigger: /shape, /spec, /plan, /cp.
argument-hint: "[idea|issue|backlog-item] [--spec-only] [--design-only]"
---

# /shape (canary)

Turn a fuzzy idea, bug, or `backlog.d/NNN-*.md` item into a **context packet**
that `/implement` can execute without re-exploring the repo. In canary, the
packet usually lands on disk as a revised `backlog.d/NNN-<kebab-slug>.md`
following the convention from `backlog.d/_done/`. For ad-hoc work it may stay
in-memory and be handed straight to `/implement`.

The product north star is **agent-native observability**: every shape must
say how an AI consumer is better served by the change, not how a human
operator clicks less. If it ends up in a dashboard, it is the wrong shape.

## Input → Output

- **Input:** raw idea, one-line bug, `backlog.d/NNN-*.md` stub, operator
  observation, or review finding.
- **Output:** a `backlog.d/NNN-*.md` file (or equivalent in-memory packet)
  containing Goal, Non-Goals, Invariants, Oracle, Dependencies, Lane,
  Priority, Estimate, and Repo Anchors. If the item needs backlog movement,
  also update `backlog.d/README.md` (priority table + dependency map).

## Workflow

### Phase 1 — Understand

Accept a raw idea, a `backlog.d/NNN-*.md` stub, an audit finding, or a
reproduction steps dump. Spawn parallel sub-agents:

- One maps the affected canary area — controllers under
  `lib/canary_web/`, domain modules under `lib/canary/`, migrations under
  `priv/repo/migrations/`, OpenAPI source under `priv/openapi/`, and nearby
  tests under `test/canary/` or `test/canary_web/controllers/`.
- One scans prior art — completed neighbors in `backlog.d/_done/`, relevant
  runbooks in `docs/` (`docs/ci-control-plane.md`,
  `docs/non-http-health-semantics.md`, `docs/api-key-rotation.md`,
  `docs/backup-restore-dr.md`, `docs/networked-service-dogfooding.md`,
  `docs/governance.md`), and the corresponding hot module from
  `CLAUDE.md`'s footgun list.
- Optionally one delegates via `/research` for outside patterns (e.g. Ramp's
  self-maintaining monitors for `#010`).

Read `VISION.md`, `PRINCIPLES.md`, and `backlog.d/README.md` for lane +
priority grounding. If the item is adjacent to a known footgun in
`CLAUDE.md` (custom string PKs like `ERR-nanoid`, Oban Lite migration,
`Req`/`:finch`+`:connect_options`, `ReadRepo` outside `ecto_repos`, Fly
endpoint `port:`, SQLite WAL delete), call it out now so the oracle covers
it.

### Phase 2 — Problem Diamond

**GATE: do not draft acceptance criteria until the problem is locked.**

1. **Five-whys the premise.** If the ticket says "add X," name the agent
   outcome underneath. "Add annotations API" is not a problem; "a triage
   agent needs a place to record its reasoning so the next agent doesn't
   redo the work" is.
2. **Responder-boundary probe.** Decide who owns the outcome. Canary owns
   ingest, health, correlation, timelines, queries, and signed webhooks.
   Repo mutation, issue creation, and LLM triage live **downstream** in
   responders (e.g. bitterblossom). If the desired outcome is "open a PR,"
   the shape is wrong — re-scope to the timeline event + webhook payload
   the responder consumes.
3. **LLM-on-request-path probe.** If the proposal needs language
   generation at request time, stop. Route it through a deterministic
   template in `lib/canary/reports/*` / `lib/canary/summary.ex`, or push
   it out to a downstream responder. No request-path LLM.
4. **Investigate and draft** the product framing: user outcome (where the
   user is an agent), affected invariants, and why this moves the north
   star. Iterate one question at a time.

### Phase 3 — Solution Diamond

**Mandatory structurally distinct alternatives — always at least two.**
Canary-flavored:

- **Option A (boundary-respecting):** emit a new timeline event + webhook
  payload; let downstream responders react. Usually the correct answer.
- **Option B (collapse into core):** fold the behavior into Canary itself.
  Name the failure mode — typically "pulls issue creation / LLM reasoning
  into the request path" or "multiplies write-path complexity against the
  single-writer SQLite pool."
- **Option C (invert a load-bearing assumption):** e.g. push-based
  heartbeats instead of HTTP probing (the `#021` shape), or
  `pull_request_target` with a trusted control-plane checkout instead of
  in-repo workflow trust (`#016`).

For each alternative sketch: architecture, files touched, pattern
alignment, effort (S/M/L/XL), and how it fails differently from the
others. Recommend one. For M+ effort, invoke the design bench in
parallel — `ousterhout` (module depth / information hiding),
`carmack` (shippability), `grug` (simplicity) — and revise on blocking
concerns. For foundation-level divergence, call `/research` or a fresh
cross-model voice.

### Phase 4 — Write the Packet

Write the backlog item. Match the canary file convention exactly.

**Filename:** `backlog.d/NNN-<kebab-slug>.md`, zero-padded three digits,
next unused number in `backlog.d/README.md`'s priority table.

**Body skeleton** (mirrors `_done/` exemplars like
`012-webhook-delivery-ledger.md`, `021-check-in-monitors-for-non-http-runtimes.md`,
`006-split-query-read-models.md`):

```markdown
# <Title>

Priority: <high|medium|low>
Status: <ready|blocked>
Estimate: <S|M|L|XL>

## Goal
<1-2 paragraph motivation. Tie to the agent-native north star or a
shipped invariant. Name the agent consumer outcome, not a UI affordance.>

## Non-Goals
- <scope exclusions — especially the responder-boundary ones,
  e.g. "Does not open PRs, file issues, or run LLM triage">
- <invariants that are deliberately out of scope for this item>

## Invariants Preserved
- <which of: `Canary.Repo` pool_size:1 single writer;
  `Canary.Health.StateMachine.transition/4` pure and table-driven;
  deterministic summaries (no request-path LLM);
  RFC 9457 Problem Details;
  scoped API keys via `:scope_ingest|:scope_read|:scope_admin`;
  OpenAPI contract at `GET /api/v1/openapi.json`;
  `ReadRepo` stays out of `ecto_repos`>

## Oracle
- [ ] Given <precondition>, when <trigger>, then <observable outcome>
- [ ] <narrow test path>, e.g. `mix test test/canary/webhooks/delivery_test.exs --trace --max-failures 3`
- [ ] `./bin/validate --strict` green (or `--fast` if scope is pre-commit)
- [ ] Coverage holds: **81%** core, **90%** `canary_sdk/` when touched

## Dependencies
- Blocks / blocked by: `#NNN`, `#MMM`
- Downstream responders (e.g. bitterblossom `bb/NNN`) if the contract is
  their input

## Lane
- <1: agent readiness | 2: contract + observability | 3: structural | 4: hardening | 5: future>

## Notes
<Why now, prior-art links, external audit citations, rejected alternatives
with failure modes. Anchor to specific `lib/canary/...` modules and
`priv/openapi/...` paths.>
```

If the item is ready to ship, set `Status: ready`. If it depends on
in-flight work, set `Status: blocked` and name the blocker in the
dependency map. Update `backlog.d/README.md`'s priority table and
dependency graph in the same commit.

### Phase 5 — Sanity-Check Against Canary Contracts

Before handing off to `/implement`, every shape answers:

- **Lane assignment.** Which of lanes 1–5 from `backlog.d/README.md`?
  If lane 1 (agent readiness), name the ramp-pattern (`#010`) or triage
  sprite (`bb/011`) outcome it unblocks.
- **Responder boundary.** Does any acceptance criterion require Canary
  to mutate a repo, open an issue, or run an LLM? If yes, rewrite the
  criterion as "emits the timeline event + webhook payload the
  responder consumes."
- **OpenAPI + router contract.** Any new HTTP surface updates
  `priv/openapi/*` and wires into `lib/canary_web/router.ex` under the
  correct scope pipeline — `:scope_ingest` (write), `:scope_read` (read),
  or `:scope_admin` (CRUD). Reference endpoint test at
  `test/canary_web/controllers/openapi_controller_test.exs`.
- **SQLite write-path consequences.** New tables go through
  `priv/repo/migrations/` only. All writes funnel through `Canary.Repo`
  (pool_size:1). `Canary.ReadRepo` is deliberately absent from
  `ecto_repos` — do not add it.
- **Identifier convention.** New entities use `PREFIX-nanoid` string PKs
  (`ERR-`, `INC-`, `WHK-`, `DLV-`, …) and set the id on the struct, not
  via `cast`. Match the custom-PK footgun in `CLAUDE.md`.
- **Self-observability.** If the change changes the agent contract or a
  delivery path, note which metric/event in `lib/canary/metrics/*` or
  timeline taxonomy (`health_check.*`, `incident.*`, `error.*`,
  `webhook.*`) proves it works in production.
- **Commit scope telegraphed.** The shape should imply the commit
  prefix: `feat(health):`, `fix(webhook):`, `refactor(query):`,
  `docs(ops):`, `chore(governance):`, `build:`.

If you cannot write an Oracle in terms of tests or HTTP behavior,
the goal is not locked. Return to Phase 2.

## Working Examples

- **Responder-boundary win:** `backlog.d/_done/012-webhook-delivery-ledger.md`
  shapes stable `DLV-*` IDs + a persistent `webhook_deliveries` ledger
  instead of pulling replay logic into consumers. Oracle cites the
  `x-delivery-id` header contract and operator-visible ledger query.
- **Invert a load-bearing assumption:** `backlog.d/_done/021-check-in-monitors-for-non-http-runtimes.md`
  inverts "HTTP probing from Canary" to push-based check-ins, keeping
  `POST /api/v1/errors` separate from liveness semantics and reusing
  the `health_check.*` event taxonomy.
- **Trusted control plane outside candidate diff:**
  `backlog.d/_done/016-immutable-ci-control-plane.md` flips CI trust to
  `pull_request_target` + `.ci/trusted/` + `.ci/candidate/` so a PR
  cannot weaken required checks by editing workflow files.
- **Pure refactor, behavior-preserving:**
  `backlog.d/_done/006-split-query-read-models.md` holds the HTTP
  contract steady while splitting `lib/canary/query.ex` into domain
  read models behind a thin `defdelegate` facade.
- **XL north-star shape still blocked:**
  `backlog.d/010-ramp-pattern.md` — note how its non-goals explicitly
  refuse full auto-merge; Canary stays the substrate.

## Canary-Specific Constraints (checklist)

- **No LLM on the request path.** Summary generation is a deterministic
  template (`lib/canary/summary.ex`, `lib/canary/reports/*`). Language
  generation belongs in a downstream responder.
- **OpenAPI contract is load-bearing.** `GET /api/v1/openapi.json` is
  the agent-self-discovery contract. New endpoints update
  `priv/openapi/*` and land under the correct router scope pipeline in
  `lib/canary_web/router.ex` (`:scope_ingest|:scope_read|:scope_admin`).
  Endpoint test: `test/canary_web/controllers/openapi_controller_test.exs`.
- **Single writer.** All write paths go through `Canary.Repo`
  (pool_size:1). New tables come from migrations under
  `priv/repo/migrations/`. `ReadRepo` stays out of `ecto_repos`.
- **Deterministic summaries + RFC 9457.** Every error response is a
  Problem Details document via `lib/canary_web/problem_details.ex`.
- **Scoped API keys.** New routes commit to one of the three scope
  pipelines. Never silently widen a write route to `:scope_read`.
- **Narrow test runs.** Acceptance cites `mix test <specific file>
  --trace --max-failures 3`, not bare `mix test`.
- **Validate gate.** Every shape expects `./bin/validate --fast` for
  pre-commit and `./bin/validate --strict` for pre-push / CI.
- **Responder boundary.** Canary never mutates repos, creates issues,
  or runs LLM triage — those live in downstream responders consuming
  signed webhooks.

## Gotchas

- **Premise unchallenged.** A ticket labelled "add X" usually encodes a
  symptom. Five-whys to the agent-consumer outcome before drafting the
  oracle. A solid shape of the wrong problem is the failure mode this
  skill exists to prevent.
- **Alternatives-in-name-only.** Three options that all sit inside the
  core service boundary are one option. Force at least one that lives
  downstream (responder) and one that inverts a load-bearing assumption
  (push vs pull, trusted vs candidate checkout, read vs write path).
- **Vague oracles.** "It should work" is not an oracle. "When a
  non-HTTP monitor misses its cadence, `mix test
  test/canary/monitors_test.exs` passes and the status endpoint
  reports `degraded`" is.
- **Missing narrow test path.** If the oracle doesn't cite a specific
  `test/canary/.../*_test.exs` file, `/implement` will either rerun the
  whole suite or guess. Cite it.
- **Silent scope pipeline choice.** A new route without a named
  `:scope_ingest|:scope_read|:scope_admin` pipeline is a shape defect.
  Pick one and name it.
- **Skipping the OpenAPI update.** New HTTP surface without a matching
  `priv/openapi/*` change breaks agent self-discovery. Treat the spec
  as a required artifact, not a follow-up.
- **LLM leaking into the request path.** If the oracle says "generate a
  summary," specify the deterministic template and files. If it needs
  real language generation, it's a responder task, not a Canary task.
- **Dashboard drift.** "Operator sees X on a page" is the wrong oracle.
  Rewrite as a query API response, a timeline event, a webhook payload,
  or a log line an agent can consume.
- **Boilerplate invariants.** Don't enumerate every invariant — only
  the ones this change touches. A shape that says "preserves all
  invariants" has preserved none.
- **Dependency silence.** If the item blocks or is blocked by another
  `#NNN`, update `backlog.d/README.md` dependency map in the same
  shape. Otherwise lane order drifts.

## Principles

- Minimize touch points — fewer modules = less blast radius against the
  single-writer pool.
- Design for deletion — downstream responder wiring beats core-service
  absorption.
- Favor existing patterns — reuse the `health_check.*` / `incident.*` /
  `webhook.*` event taxonomy before inventing a new one.
- YAGNI ruthlessly — code is a liability; every new table, endpoint,
  and GenServer fights for its life.
- Recommend one alternative, don't just list. Shapes that end in "TBD"
  are not shapes.
- One question at a time during Phase 2/3 discussion. No batched
  clarifications.
