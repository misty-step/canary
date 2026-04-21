---
name: code-review
description: |
  Parallel multi-agent code review. Launch reviewer team, synthesize findings,
  auto-fix blocking issues, loop until clean.
  Use when: "review this", "code review", "is this ready to ship",
  "check this code", "review my changes".
  Trigger: /code-review, /review, /critique.
argument-hint: "[branch|diff|files]"
---

# /code-review

Penultimate gate in Canary's outer loop: `/settle → /refactor → /code-review → merge`.
You are the marshal — read the diff, dispatch the installed reviewer panel in
parallel, synthesize findings against Canary's invariants, fix blockers, loop
until `./bin/validate --strict` is green and every invariant holds.

## Marshal Protocol

1. **Read the diff.** `git diff master...HEAD` and `git diff --name-only master...HEAD`.
   Classify: router surface, schemas, hot-path modules, webhooks, OpenAPI,
   migrations, Dagger lanes, docs/runbooks, SDK (`canary_sdk/` or `clients/typescript/`).

2. **Select the reviewer panel (installed roster only).** The installed agents
   under `.claude/agents/` are: `beck`, `builder`, `carmack`, `critic`, `grug`,
   `ousterhout`, `planner`. Panel is always `critic` + 2-4 others chosen from
   the diff classification. Never invent personas; never call a non-installed
   agent. The selection algorithm in `references/bench-map.yaml` is the
   fallback; overrides are documented in the synthesis output.

   Canary-specific routing (union with defaults):

   | Changed surface | Add reviewers |
   |---|---|
   | `lib/canary/query.ex`, `lib/canary/incidents.ex`, module splits (#125-style) | `ousterhout` |
   | `lib/canary/errors/ingest.ex`, `lib/canary/webhooks/delivery.ex`, `lib/canary/alerter/*.ex` | `carmack` |
   | New modules, new GenServers, new abstractions anywhere | `grug` |
   | `test/**`, `lib/canary/health/state_machine.ex`, changes near coverage thresholds | `beck` |
   | `lib/canary_web/router.ex`, `priv/openapi/*`, `lib/canary_web/problem_details.ex`, `lib/canary/schemas/*.ex`, `lib/canary/webhooks/*.ex` | `ousterhout` + `carmack` |
   | `priv/repo/migrations/*` | `carmack` + `beck` |
   | `dagger/**`, `.github/workflows/ci.yml`, `bin/validate`, `bin/dagger` | `carmack` + `critic` |

3. **Dispatch the panel in parallel.** Each reviewer is a separate sub-agent
   with a tailored prompt citing exact file paths and the Canary invariant
   checklist below. Same-agent self-review is forbidden — `builder` never
   reviews its own code.

   | Reviewer | Lens | What to hunt on Canary |
   |---|---|---|
   | `ousterhout` | Deep modules, information hiding | Shallow pass-throughs in `lib/canary/query.ex` read-model split; public surface that leaks internal schema; module boundaries around `Canary.Incidents`, `Canary.Timeline`, `Canary.IncidentCorrelation`. |
   | `carmack` | Direct implementation, hot-path perf, shippability | `lib/canary/errors/ingest.ex` (ingest hot path — no N+1, no blocking network calls); `lib/canary/webhooks/delivery.ex` async retry + signer; `lib/canary/alerter/circuit_breaker.ex` ETS semantics; `lib/canary/health/manager.ex` boot `rescue`. |
   | `grug` | Complexity-demon hunter | Over-abstracted behaviours, speculative GenServers, any hint of LLM-on-request-path (invariant violation — reject immediately). Deterministic template summaries only. |
   | `beck` | TDD + simple design | Table-driven tests for `Canary.Health.StateMachine.transition/4` (pure), coverage relative to **81%** (core) and **90%** (`canary_sdk/`), tests next to the change, narrow `mix test` runs. |
   | `critic` | Skeptical evaluator against grading criteria | Pinned on every panel. Emits structured pass/fail with explicit objections mapped to the invariant checklist. |

   Supplementary tiers (Thinktank, cross-harness Codex/Gemini) remain available
   per `references/thinktank-review.md` and `references/cross-harness.md`. Wait
   for `trace/summary.json` to reach `complete` or `degraded` before consuming.

4. **Invariant checklist — every reviewer applies this to their file slice.**

   - Ecto custom string PKs (`ERR-nanoid`, `INC-nanoid`, `WHK-nanoid`) set on
     the struct, never via `cast/3`. Pattern: `%Error{id: id} |> changeset(attrs)`.
     Casting silently drops `id` — six prior bugs. Grep for any `cast(.*:id.*)`
     touching these schemas.
   - `Canary.Repo` pool_size:1 preserved. `Canary.ReadRepo` stays OUT of
     `config :canary, ecto_repos: [...]`. Only `Canary.Repo` runs migrations.
   - `Canary.Health.StateMachine.transition/4` stays pure — no `Repo`, no
     `GenServer`, no `Logger`, no clock side effects. Table-driven tests.
   - Summary generation deterministic templates. **No LLM on the request path.**
     This is an invariant, not a preference. Any call to Anthropic/OpenAI/etc
     from an ingest, webhook, or query code path is an automatic block.
   - RFC 9457 Problem Details for all error responses — every new error path
     routes through `lib/canary_web/problem_details.ex`. No ad-hoc `{:error, "..."}`
     JSON leaks.
   - Router pipelines use the correct scope: `:scope_ingest` for ingest,
     `:scope_read` for queries, `:scope_admin` for mutations. Cross-scope leaks
     are a security finding.
   - OpenAPI under `priv/openapi/*` updated whenever router surface changes —
     the spec is the agent contract (`GET /api/v1/openapi.json`), not a nice-to-have.
   - Responder boundary: no repo mutation, no issue creation, no LLM triage in
     this repo. Those live in downstream responders (bitterblossom etc.).
   - Webhook payload changes are a product contract break — treat as major
     version and flag in synthesis.
   - Oban Lite tables created ONLY via the dedicated migration
     `priv/repo/migrations/20260314230000_create_oban_jobs.exs`. Never in a
     GenServer, `Application.start/2`, or Release module (races pool_size:1).
   - `Req.request/1` never passes both `:finch` and `:connect_options` — use
     `:receive_timeout`.
   - Fly endpoint `http:` block in `config/runtime.exs` keeps explicit `port:`.

5. **Synthesize.** Deduplicate across reviewers. Rank by severity:
   **blocking** (invariant violation, correctness, security, webhook contract
   break) > **important** (architecture, testing gap, shallow module) >
   **advisory** (style, naming).

6. **Verdict.**
   - No blocking findings AND `./bin/validate --strict` green → **Ship**.
   - Blocking findings present → enter Fix Loop.
   - Pending advisories only → **Conditional**, document in the verdict.

## Fix Loop

For each blocking finding, spawn a `builder` sub-agent with the specific
`file:line` plus the invariant it violates. Builder fixes strategically
(Ousterhout), simply (grug), runs the narrow test first:

```bash
mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3
```

Then commits with the project's conventional-with-scope prefix
(`fix(health):`, `refactor(query):`, `fix(ci):`, etc.). After all blocking
fixes land, **re-dispatch the full panel** — not a spot-check. Loop until
clean. Max 3 iterations; then escalate to the human.

## Gate Verification

Before any **Ship** verdict, the reviewer marshal confirms both layers pass:

- Local: `./bin/validate --strict` → `dagger call strict` (full gate + live
  advisory scan + `.codex/agents/*.toml` role validation; pre-push hook).
- Hosted: `.github/workflows/ci.yml` runs `dagger call strict --source=../candidate`
  from the immutable `pull_request_target` control plane documented in
  `docs/ci-control-plane.md`. Trusted base snapshot at `.ci/trusted/`, candidate
  at `.ci/candidate/`. Both layers share the same Dagger entrypoint — if strict
  is green locally and the control plane is unchanged, CI is green.

Never substitute `mix test` or bare `dagger call` for `./bin/validate --strict`.
The deterministic package gates inside strict are: core (compile, format,
credo, sobelow, coverage **81%**, dialyzer), `canary_sdk/` (compile, format,
coverage **90%**), TS SDK (typecheck, coverage, build).

## Plausible-but-Wrong Patterns (Canary flavor)

LLMs optimize for plausibility. On Canary, the highest-leverage traps:

- Custom-PK schema that *looks* right but uses `cast/3` — will silently lose
  the `ERR-nanoid` id and the row gets a random Ecto-generated key.
- New GenServer that creates or migrates a SQLite table on boot — races the
  pool_size:1 migrator.
- "Summary generator" that reaches for an LLM client because the templates
  feel limiting — invariant violation, reject.
- Webhook payload field added/removed/renamed without a version bump —
  downstream responders break silently.
- Router route added to the wrong pipeline (`:scope_read` mutation, or
  ingest leaked into admin).
- Shallow module with one public function that forwards to one private
  function — Ousterhout's pass-through smell.
- Test that mocks `StateMachine.transition/4` instead of table-driving it —
  defeats the purity invariant.
- `Req.request/1` call with `:finch` AND `:connect_options` — silent
  timeout behaviour.

## Simplification Pass

After panel clears, if diff > 200 LOC net, sweep for:

- Pass-throughs collapsible into the caller.
- `lib/canary/query.ex` read-model split (#125) leaving dead helpers.
- Speculative behaviours with one implementation.
- Compat shims for webhook payload shapes nobody consumes.
- Docs duplicated across `docs/` runbooks and module docstrings — pick one.

## Review Scoring

Append one JSON line to `.groom/review-scores.ndjson` (create `.groom/` if
absent). Committed to git so `/groom` can read trends.

```json
{"date":"2026-04-20","pr":125,"correctness":8,"depth":7,"simplicity":9,"craft":8,"verdict":"ship","providers":["claude-panel","thinktank","codex"]}
```

- `pr` = PR number, or `null` for a branch without a PR.
- `verdict` ∈ `ship | conditional | dont-ship`.
- `providers` = which tiers actually contributed.

## Verdict Ref

If `scripts/lib/verdicts.sh` exists, record a verdict ref so `/settle --land`
can gate on it without needing a GitHub PR:

```bash
source scripts/lib/verdicts.sh
verdict_write "<branch>" '{"branch":"<branch>","base":"master","verdict":"<ship|conditional|dont-ship>","reviewers":[...],"scores":{...},"sha":"<git rev-parse HEAD>","date":"<ISO-8601>"}'
```

- Write on every verdict, not only ship — `dont-ship` blocks `/settle --land`.
- `sha` MUST be `git rev-parse HEAD` at review time. New commits invalidate
  the verdict; `/settle` re-runs review.
- Mirror to `.evidence/<branch>/<date>/verdict.json`.
- The `SPELLBOOK_NO_REVIEW=1` escape hatch is owned by the caller, not here.

Skip if `scripts/lib/verdicts.sh` is absent (Spellbook-only feature).

## Outer Loop Position

`/code-review` is the penultimate gate before `/settle` lands the branch.
Expected upstream state: `/settle` has unblocked CI + reviews, `/refactor`
has already simplified the diff. Expected downstream: a clean verdict ref
and a green `./bin/validate --strict` so `/settle --land` can merge into
`master` (linear history, no squash). Commit style is conventional-with-scope:
`feat(health):`, `fix(ci):`, `refactor(query):`, `docs(ops):`.

## Gotchas

- **Self-review leniency:** `builder` never reviews its own code. Panel
  agents are separate sub-agents.
- **Reviewing the whole repo:** Scope is `git diff master...HEAD`, not the
  tree. Canary is small enough that drift is tempting; resist.
- **Skipping the invariant checklist:** Every reviewer applies it to their
  file slice. A clean-looking panel that skipped the checklist is a false
  green — six prior PK bugs shipped exactly this way.
- **Invented reviewers:** The installed roster is `beck`, `builder`,
  `carmack`, `critic`, `grug`, `ousterhout`, `planner`. Nothing else.
- **Treating advisories as blockers:** Style/naming do not gate ship.
  Invariant violations do.
- **Trusting `mix test` alone:** Ship gate is `./bin/validate --strict`.
  Narrow `mix test` runs are for the inner loop.
- **Forgetting the agent contract:** Router surface changes without
  `priv/openapi/*` updates break agent consumers — always blocking.
- **LLM-on-request creep:** Any request-path call to an LLM provider is an
  automatic block, even if tests pass. This is the product's north star.
