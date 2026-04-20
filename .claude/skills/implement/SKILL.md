---
name: implement
description: |
  Atomic TDD build skill. Takes a context packet (shaped ticket) and
  produces code + tests on a feature branch. Red → Green → Refactor.
  Does not shape, review, QA, or ship — single concern: spec to green tests.
  Use when: "implement this spec", "build this", "TDD this", "code this up",
  "write the code for this ticket", after /shape has produced a context packet.
  Trigger: /implement, /build (alias).
argument-hint: "[context-packet-path|ticket-id]"
---

# /implement

Spec in, green tests out. One packet, one feature branch, one concern — for
the Canary core service (Phoenix 1.8 + Ecto + `ecto_sqlite3`).

## Invariants

- Trust the context packet. Do not reshape. Do not re-plan.
- If the packet is incomplete, **fail loudly** — do not invent the spec.
- Honor the Canary invariants at all times (see "Canary invariants" below).
  The spec does not get to override them; if it tries, stop.

## Contract

**Input.** A context packet: goal, non-goals, constraints, repo anchors,
oracle (executable preferred), implementation sequence. Canary packets live
in `backlog.d/` — e.g. `backlog.d/010-ramp-pattern.md`. Resolution order:

1. Explicit path argument (`/implement backlog.d/010-ramp-pattern.md`)
2. Backlog item ID (`/implement 010`) → resolves via `backlog.d/<id>-*.md`
3. Last `/shape` output in the current session
4. **No packet found → stop.** Do not guess the spec from a title.

Required packet fields (hard gate — missing any = stop):
- `goal` (one sentence, testable)
- `oracle` (how we know it's done, ideally executable commands — in Canary
  that usually means `mix test test/canary/<area>/<area>_test.exs` plus
  `./bin/validate --fast`)
- `implementation sequence` (ordered steps, or explicit "single chunk")

See `references/context-packet.md` for the full shape.

**Output.**
- Code under `lib/canary/<area>/` + tests under `test/canary/<area>/` on a
  feature branch (`<type>/<slug>` from current branch)
- All tests green: narrow area test + `./bin/validate --fast`
- Working tree clean (no `IO.inspect`, no `dbg/1`, no scratch files, no
  stray migrations)
- `mix format` applied
- Commits in repo convention — one logical unit per commit, conventional
  with scope: `feat(health):`, `fix(webhooks):`, `refactor(query):`, etc.
- Final message: branch ref + oracle checklist status

**Stops at:** green narrow test + `./bin/validate --fast` green + clean
tree. Does not run `./bin/validate --strict`, `/code-review`, `/qa`, or open
a PR — `--strict`, review, and landing belong to `/settle` / `/deliver`.

## Workflow

### 1. Load and validate packet

Resolve the packet (order above). Parse required fields. If any are missing
or vague ("add annotations API" with no oracle commands), stop with:

> Packet incomplete: missing <field>. Run /shape first.

Do not try to fill in the gaps. Shape is a different skill's judgment.

### 2. Create the feature branch

`git checkout -b <type>/<slug>` from the current branch. Use the commit-scope
vocabulary for `<type>`: `feat`, `fix`, `refactor`, `chore`, `docs`, `build`.
Builders never commit to `master`. If you forget, create the branch after
and cherry-pick before handing off. Master is linear — do not squash,
do not merge commits on top.

### 3. Dispatch the builder

Spawn a **builder** sub-agent (general-purpose) with:
- The full context packet
- The executable oracle (narrow `mix test` + `./bin/validate --fast`)
- The TDD mandate (see below)
- The Canary invariants (below) — handed over literally, not summarized
- File ownership (if the packet decomposes into disjoint chunks, spawn
  multiple builders in parallel — one per chunk, each with subset of oracle)

**Builder prompt must include:**
> You MUST write a failing test before production code. RED → GREEN →
> REFACTOR → COMMIT. Exceptions: Ecto migrations under `priv/repo/migrations/`,
> config (`config/*.exs`), generated code, LiveView template HEEX layout.
> Document any skipped-TDD step inline in the commit message.

See `references/tdd-loop.md` for the full cycle and skip rules.

### 4. TDD loop (Red → Green → Refactor)

**Red.** Write the failing test first. Put it at
`test/canary/<area>/<area>_test.exs` (mirror the module path — e.g.
`lib/canary/health/state_machine.ex` → `test/canary/health/state_machine_test.exs`).
Follow the existing layout: `use ExUnit.Case, async: true` for pure
modules, `use Canary.DataCase` for anything that touches `Canary.Repo`,
`use CanaryWeb.ConnCase` for controllers. Fixtures live in
`test/support/fixtures.ex`. Run it narrow:

```bash
mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3
```

This is the user-ratified default — not a bare `mix test`. `--trace` keeps
output ordered so a failing assertion is easy to attribute; `--max-failures 3`
bails after the first cluster so you're not reading 40 cascading failures.

**Green.** Write the minimum code in `lib/canary/<area>/` to pass the test.
Re-run the same narrow command. Do not widen scope until the single
behavior is green.

**Refactor.** Local cleanup only — no broader simplification (that's
`/refactor`'s job).

```bash
mix format
mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3
./bin/validate --fast
```

`./bin/validate --fast` runs `dagger call fast` — the pre-commit subset
(lint + core tests). It's wired via `.githooks/pre-commit`, so what you
run here is what the hook will run. If it passes, commit. If not, diagnose
from its output before widening.

Optional direct probes (all also run inside `--fast` / `--strict`):
`mix credo`, `mix sobelow`, `mix dialyzer`.

### 5. Verify exit conditions

Before exiting, confirm:
- [ ] `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3` exits 0
- [ ] `./bin/validate --fast` exits 0
- [ ] `mix format --check-formatted` clean
- [ ] `git status` clean (no untracked scratch files, no stray migration)
- [ ] No `IO.inspect`, `dbg(`, `IO.puts("here")`, or `TODO`/`FIXME` added
      that isn't in the spec
- [ ] Every oracle command from the packet exits 0 (run them, don't trust
      the builder)
- [ ] Commits are logically atomic (one concern per commit) and use the
      conventional-with-scope prefix (`feat(<area>):`, `fix(<area>):`,
      `refactor(<area>):`)

If any check fails, dispatch a builder sub-agent to fix. Max 2 fix loops,
then escalate.

### 6. Hand off

Output: feature branch name, commit list, oracle checklist (which commands
pass), residual risks. Do not run `/code-review`, do not run
`./bin/validate --strict`, do not push, do not open a PR unless the packet
explicitly says so. `--strict` (full coverage gates, advisories, dialyzer)
is the pre-push gate (`.githooks/pre-push`) and belongs to `/settle`.

## Canary invariants (non-negotiable)

Every implementation honors these. The spec does not get to override them.

- **Custom string PKs must be set on the struct, never cast.** IDs are
  `ERR-nanoid`, `INC-nanoid`, `WHK-nanoid`, `MON-nanoid`. Ecto's `cast/3`
  silently drops the `id` field because it isn't in `@required`/`@optional`
  — this bug bit the initial build six times. Always:

  ```elixir
  id = "ERR-" <> Nanoid.generate()
  %Error{id: id} |> Error.changeset(attrs) |> Canary.Repo.insert()
  ```

  Never `Error.changeset(%Error{}, Map.put(attrs, :id, id))`. Same rule for
  `%Incident{id: id}`, `%Webhook{id: wh_id}`, `%Monitor{id: id}`. Grep for
  this pattern: `lib/canary/errors/ingest.ex`, `lib/canary/incidents.ex`,
  `lib/canary/monitors.ex`, `lib/canary_web/controllers/webhook_controller.ex`.

- **RFC 9457 Problem Details for every error response.** Use
  `CanaryWeb.Plugs.ProblemDetails.render_error(conn, status, code, detail, extra)`
  at `lib/canary_web/plugs/problem_details.ex`. Content type is
  `application/problem+json`. Codes: `invalid_request`, `invalid_api_key`,
  `insufficient_scope`, `not_found`, `payload_too_large`, `validation_error`,
  `rate_limited`, `internal_error`, `unavailable`. Do not hand-roll
  `%{error: "..."}` JSON.

- **Pure state machine.** `Canary.Health.StateMachine.transition/4`
  (`lib/canary/health/state_machine.ex`) is pure — no side effects,
  no `Repo` calls, no clock reads beyond the flap-window monotonic time.
  Output is `{new_state, counters, side_effects}`; the caller executes the
  effects. Tests are table-driven (see
  `test/canary/health/state_machine_test.exs`) with `async: true`. Any
  new state or event goes through `transition/4`; do not branch side
  effects into the state function.

- **Single writer.** All DB writes go through `Canary.Repo` (pool_size: 1,
  SQLite single-writer). `Canary.ReadRepo` is read-only and deliberately
  absent from `config :canary, ecto_repos` — adding it there makes
  `mix ecto.migrate` hunt for `priv/read_repo/migrations/` and blow up.
  Reads fan out through `Canary.ReadRepo`; writes never do.

- **Deterministic summaries.** Summaries live under `lib/canary/reports/*`
  and `lib/canary/*/summary.ex` as pure template functions. No LLM,
  no network, no stochastic output on the request path. Agents call the
  API, Canary returns structured+summarized facts.

- **Responder boundary.** Canary owns ingest, health, incident correlation,
  timelines, query APIs, and signed webhooks. Repo mutation, issue creation,
  and LLM triage are *out of scope* — they belong in downstream responders.
  If the packet asks you to open a GitHub issue from Canary, stop.

- **Narrow test idiom.** Run area tests with
  `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3`.
  Not bare `mix test`. The full suite runs via `./bin/validate --fast`.

## Scoping Judgment (what the model must decide)

- **Test granularity.** One behavior per test. If you can't name the
  behavior in one short sentence (e.g. "transitions `:up` to `:degraded`
  after `degraded_after` failures"), the test is too big.
- **When to skip TDD.** Ecto migrations (`priv/repo/migrations/*.exs`),
  config (`config/*.exs`, `config/runtime.exs`), generated code, LiveView
  template layout. Document the skip in the commit message. Everything
  else: test first.
- **When to escalate.** Builder loops on the same test failure 3+ times,
  the oracle contradicts the constraints, the spec requires behavior that
  violates a Canary invariant (PK cast, LLM on request path, writes
  outside `Canary.Repo`, hand-rolled error shape), or a migration-adjacent
  bug surfaces (Oban Lite tables, `ReadRepo` in `ecto_repos`, SQLite WAL
  on a live machine). Stop and report, don't power through.
- **Parallelism.** Only parallelize when file ownership is disjoint and
  oracle criteria partition cleanly. Shared files (`lib/canary_web/router.ex`,
  `lib/canary/query.ex`) → serial builders.
- **Refactor depth.** The refactor step in TDD is local — improve the
  code you just wrote. Broader refactors (pass-through layers, shallow
  modules) are `/refactor`'s job, not yours.

## What /implement does NOT do

- Pick tickets (caller's job, or `/deliver` / `/flywheel`)
- Shape or re-shape specs (→ `/shape`)
- Code review (→ `/code-review`)
- QA against a running `canary-obs` (→ `/qa`)
- Full CI gate / coverage / advisories (→ `/ci`, via `./bin/validate --strict`)
- Simplification beyond TDD-local refactor (→ `/refactor`)
- Ship, merge, deploy (`flyctl deploy --app canary-obs --remote-only`,
  landing a PR) — → human, `/settle`, or `/flywheel`

## Stopping Conditions

Stop with a loud report if:
- Packet is incomplete or ambiguous
- Oracle is unverifiable (prose-only checkboxes with no `mix test` /
  `./bin/validate` commands — write one, or stop)
- Builder fails the same test 3+ times after targeted fix attempts
- Spec contradicts itself or violates a Canary invariant
- Tests need a live dependency the sandbox can't provide (network,
  Fly-only storage, a running `canary-obs` machine)

**Not** stopping conditions: spec is hard, unfamiliar area, initial tests
red, coverage below 81% on first green (address in the refactor step or
by widening tests — that's the job).

## Gotchas

- **PK drop via `cast/3`.** Writing `Error.changeset(%Error{}, Map.put(attrs, :id, id))`
  silently drops `id`. Repo inserts a row with a SQLite-generated rowid-as-id
  or fails on null. Always `%Error{id: id} |> Error.changeset(attrs)`.
- **`ReadRepo` creep.** Do not add `Canary.ReadRepo` to `config :canary, ecto_repos`
  in `config/config.exs`. If you need a read-only query, call `Canary.ReadRepo.all/1`
  directly.
- **Oban migrations.** Oban Lite does not auto-create its tables. The
  project's Ecto migration at `priv/repo/migrations/20260314230000_create_oban_jobs.exs`
  owns this. Never create Oban tables from a GenServer or Release module
  — `Repo.query!` races `Ecto.Migrator` with pool_size:1.
- **`Req` options.** Never pass both `:finch` and `:connect_options` to
  `Req.request/1`. Use `:receive_timeout` for timeouts. `Canary.Webhooks.Delivery`
  is the reference.
- **Hand-rolled error JSON.** Using `json(conn, %{error: "nope"})` bypasses
  RFC 9457. Route through `CanaryWeb.Plugs.ProblemDetails.render_error/5`.
- **Reshaping inside /implement.** If the spec is wrong, stop. Don't
  silently rewrite the oracle to match what you built.
- **Declaring victory with partial oracle.** "Narrow test green" is not
  done — `./bin/validate --fast` must also be green.
- **Silent catch-and-return.** New code that rescues and returns a fallback
  is hiding bugs. Fail loud. Test the failure mode. `Health.Manager`'s
  `rescue` in `handle_info(:boot)` is a documented exception, not a
  pattern to copy.
- **Testing implementation.** Tests that assert the internal structure of
  `Canary.Health.StateMachine` (private function names, counter field
  ordering) break on every refactor. Test the `{state, counters, effects}`
  tuple from the outside.
- **Committing debug noise.** `IO.inspect`, `dbg()`, `IO.puts("here")`,
  commented-out code. The tree must be clean before exit.
- **Skipping `mix format`.** `./bin/validate --fast` will fail the format
  check. Run `mix format` before committing, always.
- **Parallelizing coupled builders.** Two builders editing
  `lib/canary_web/router.ex` or `lib/canary/query.ex` at once will merge-conflict
  and lose work. Partition by file ownership first.
- **Branch drift.** Forgetting to `git checkout -b feat/<slug>` and
  committing to `master`. Master is linear — always branch first.
- **Scope creep from builders.** Builder adds "while I'm here" cleanups
  to `lib/canary/errors/ingest.ex`. The spec is the constraint — raise a
  blocker, don't silently expand the diff.
- **Trusting self-reported success.** Builders say "all tests pass." Verify
  by running the narrow test and `./bin/validate --fast` yourself. Agents
  lie (accidentally).
