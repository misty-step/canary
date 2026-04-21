---
name: diagnose
description: |
  Investigate, audit, triage, and fix. Systematic debugging, incident lifecycle,
  domain auditing, and issue logging. Four-phase protocol: root cause → pattern
  analysis → hypothesis test → fix.
  Use for: any bug, test failure, production incident, error spikes, audit,
  triage, postmortem, "diagnose", "why is this broken", "debug this",
  "production down", "is production ok", "audit stripe", "log issues".
  Trigger: /diagnose.
argument-hint: <symptoms or domain> e.g. "error in auth" or "audit stripe"
---

# /diagnose

Find root cause. Fix it. Prove it works. Canary is an observability substrate
for agents, deployed as Fly app `canary-obs` on SQLite + Litestream. The
failure modes below are not generic — they are the footguns baked into this
repo's `CLAUDE.md` and `AGENTS.md`. Start from them.

## Execution Stance

You are the executive orchestrator.
- Keep hypothesis ranking, root-cause proof, and fix selection on the lead model.
- Delegate bounded evidence gathering and implementation to focused subagents.
- Run parallel hypothesis probes when multiple plausible causes exist.

## Routing

| Intent | Sub-capability |
|--------|---------------|
| Debug a bug, test failure, unexpected behavior | This file (below) |
| Flaky test investigation | `references/flaky-test-investigation.md` |
| Incident lifecycle: triage, investigate, postmortem | `references/triage.md` |
| Domain audit: "audit ingest", "audit webhooks", "audit health" | `references/audit.md` |
| Audit then fix highest priority issue | `references/fix.md` |
| Create GitHub issues from audit findings | `references/log-issues.md` |

If first argument matches a canary domain (ingest, health, webhooks, incidents,
timeline, query), route to `references/audit.md`.
If "triage", "incident", "postmortem", "production down", "canary-obs is down"
→ `references/triage.md`.
If "flaky", "flake", "intermittent", "nondeterministic test" →
`references/flaky-test-investigation.md`.
If "fix" → `references/fix.md`. If "log issues" → `references/log-issues.md`.
Otherwise, this is a debugging session — continue below.

**The user's symptoms:** $ARGUMENTS

## The Iron Law

```
NO FIXES WITHOUT ROOT CAUSE INVESTIGATION FIRST
```

If you haven't completed Phase 1, you cannot propose fixes.

## Rule #1: Check the Canary Footguns Before Reading Code

Canary has a codified footgun list. Most "mysterious" failures are one of
them. Walk this checklist before opening a module:

1. **Ecto custom string PK dropped?** If an `ERR-…`, `INC-…`, `WHK-…`, or
   `TGT-…` row fails to persist or comes back with a generated UUID, the
   changeset is using `cast/3` on `:id`. The invariant is `%Error{id: id} |>
   Error.changeset(attrs)` — set the PK on the struct, never in the cast list.
   Prior art: six bugs in the initial build. Reference:
   `lib/canary/errors/ingest.ex` (the `%Error{id: error_id}` literal at the
   top of `do_ingest/1`) and `lib/canary/health/manager.ex` (`%Target{id: id}`
   in `handle_call({:add_target, …})`).
2. **Oban table missing?** If the app crashes on boot with
   `no such table: oban_jobs`, `mix ecto.migrate` did not run
   `priv/repo/migrations/20260314230000_create_oban_jobs.exs`. Never create
   the table from a GenServer or Release module — `Repo.query!` races the
   single-writer pool. Fix is to run the migration; do not work around it.
3. **Req/Finch option conflict?** An `ArgumentError` from `Req.request/1`
   mentioning `:finch` and `:connect_options` means someone passed both.
   Strip `:connect_options`; use `:receive_timeout` for the timeout knob.
   Hot spot: `lib/canary/webhooks/delivery.ex` / webhook worker call sites.
4. **`ReadRepo` in `ecto_repos`?** If `mix ecto.migrate` complains about a
   missing `priv/read_repo/migrations/` directory, someone added
   `Canary.ReadRepo` to `config :canary, ecto_repos:`. Remove it. Only
   `Canary.Repo` runs migrations; `ReadRepo` is read-only by design.
5. **`Health.Manager` boot log noisy?** A `Health manager boot failed: …
   retrying in 5s` warning on boot is *expected behaviour*, not a bug — see
   the `rescue` clause in `handle_info(:boot, state)` in
   `lib/canary/health/manager.ex`. It covers Ecto sandbox start-up and prod
   boot races where the `targets` table isn't visible yet.
6. **Fly machine bound to random port?** If the Fly health check fails with
   "instance refused connection" and the proc listens on something other
   than `8080`, check `config/runtime.exs` for a second
   `config :canary, CanaryWeb.Endpoint` block that replaces (not merges) the
   `http:` keyword list without `port:`. The prod block must explicitly
   include `port:`.
7. **Tried to clear prod DB with `rm -f` and nothing changed?** SQLite WAL
   keeps the file handle open while BEAM is running. The documented
   stop → mount-volume → rm → start sequence lives in
   `docs/backup-restore-dr.md`. Do not improvise — that doc is the only
   tested recovery path.

Only after all seven are ruled out should you open code.

## Sub-Agent Patterns

### Quick investigation (default)

Spawn a single **Explore** subagent to gather evidence. Tell it to investigate
the symptoms, reproduce the issue, trace data flow, and report back with root
cause + evidence + proposed fix. It should NOT implement the fix — just report.
You review, decide if root cause is proven, then dispatch a **builder** for
the fix or dig deeper.

### Multi-Hypothesis Mode

When >2 plausible root causes and a single investigation would anchor on one:
spawn parallel **Explore** subagents, one per hypothesis. Each gets one
hypothesis to prove or disprove by tracing a specific subsystem. They report
back with confirmed/disproved + evidence. You synthesize into a consensus root
cause, then dispatch a **builder** (general-purpose) for the fix.

Use when: ambiguous stack trace spanning ingest + correlation + webhook
delivery; `canary-obs` misbehaving across Repo + Oban + Finch; a flaky
`test/canary/health/manager_test.exs`.
Don't use when: the symptom matches one of the seven footguns above — that
is a single-subagent job at most.

### What you keep vs what you delegate

| You (lead) | Sub-agents (investigators) |
|------------|---------------------------|
| Ranking hypotheses | Tracing one subsystem (ingest, health, webhooks) |
| Declaring root cause proven | Comparing working vs broken changeset paths |
| Choosing the fix | Gathering Fly logs, reproductions, `mix test` output |
| Deciding when evidence is sufficient | Running targeted test cases |

## Instrumented Reproduction Loop

When you can't reproduce the bug yourself (Fly-only behaviour, timing-dependent
Oban races, webhook delivery against real remote endpoints, Litestream restore
paths):

```
INSTRUMENT → USER REPRODUCES → READ LOGS → REFINE → REPEAT
```

1. **Hypothesize** — form 2-3 candidate root causes from the symptom, ranked
   against the seven-footgun checklist.
2. **Instrument** — add `Logger.info/2` at decision points. Tag each line
   with the hypothesis it tests:
   `Logger.info("[H1] changeset id=#{inspect(changeset.data.id)}")`.
   For prod, stream via `flyctl logs --app canary-obs` — do not write to a
   local file.
3. **Hand off** — tell the user: "Reproduce against `canary-obs` (or run the
   narrow test), then say done." Give the exact `curl` / `mix test` command
   if known.
4. **Read & analyze** — read the Fly log tail or local test output. For
   each hypothesis: supported / disproved / insufficient data.
5. **Iterate** — max 3 rounds. If still ambiguous, escalate to
   Multi-Hypothesis Mode.
6. **Clean up** — remove all `Logger.info` instrumentation before the final
   fix commit. Instrumentation is diagnostic, not the patch.

Use when: behaviour only reproduces on `canary-obs`, or only under a real
webhook receiver, or only after Litestream restore.
Don't use when: it reproduces under `mix test` locally.

## The Four Phases

### Phase 1: Root Cause Investigation

BEFORE attempting ANY fix:

1. **Read error messages carefully** — full stack traces, line numbers, error
   codes. Ecto changeset errors look like validation errors but a missing
   `id` field usually means rule #1 above.
2. **Reproduce with the narrowest test possible** —
   `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3`.
   Never run bare `mix test` as a diagnostic first step; it buries the
   failing assertion.
3. **Check recent changes** — `git diff`, `git log --oneline -20`,
   `git log --grep 'fix('` for past patches in the same area. If the bug
   touches a hot module
   (`lib/canary/errors/ingest.ex`, `lib/canary/incidents.ex`,
   `lib/canary/health/manager.ex`, `lib/canary/health/state_machine.ex`,
   `lib/canary/webhooks/delivery.ex`, `lib/canary/alerter/circuit_breaker.ex`,
   `lib/canary_web/router.ex`), read the most recent commits touching it.
4. **Gather evidence across component boundaries** — ingest →
   group upsert → webhook enqueue → Oban → Finch delivery. Log at each
   boundary, run once, identify the failing layer.
5. **Trace data flow backward** — where did the bad value originate? For an
   ingest bug, start at `Canary.Errors.Ingest.ingest/1` and trace down. For
   a delivery bug, start at the timeline event, follow
   `Canary.Workers.WebhookDelivery.enqueue_for_event/2` into
   `lib/canary/webhooks/delivery.ex`.

### Phase 2: Pattern Analysis

1. **Find working examples in this repo** — how does the ingest path set its
   PK vs. how does the health manager set its PK? Both should use the
   struct-literal form. Any deviation is suspicious.
2. **Check `backlog.d/_done/`** — items like `backlog.d/_done/004-incident-
   correlation-failure-paths.md`, `backlog.d/_done/012-webhook-delivery-
   ledger.md`, and `backlog.d/_done/013-self-observability-metrics.md`
   describe prior fixes in the exact areas most bugs touch. Read them before
   re-deriving a fix.
3. **Query canary for prior occurrences** — `Canary.ErrorReporter` is wired
   as a `:logger` handler and direct-ingests via
   `Canary.Errors.Ingest.ingest/1` (no HTTP loopback). If the symptom has
   happened before, it is already in the `errors` table. Query
   `GET /api/v1/errors?service=canary&error_class=<class>` with the
   `read-only` scoped API key.
4. **Check circuit breaker state for webhook issues** — suspended deliveries
   live in the ETS table owned by `lib/canary/alerter/circuit_breaker.ex`:
   10-failure threshold, 5-minute probe interval. ETS is process-local; a
   `flyctl machines restart <id> --app canary-obs` is the only way to clear
   it without code changes.
5. **Understand config vs. code** — env vars (`DATABASE_PATH`,
   `BUCKET_NAME`, `AWS_ACCESS_KEY_ID`, `BOOTSTRAP_API_KEY`), `config/runtime.exs`,
   `litestream.yml`. Check `flyctl secrets list --app canary-obs` before
   reading code.

### Phase 3: Hypothesis and Testing

Scientific method. One experiment at a time. No stacking.

1. **Form single hypothesis** — "I think X causes Y because Z" (write it
   down explicitly). Name the suspected footgun or module by path.
2. **Design experiment** — the smallest test that discriminates. For an
   ingest PK bug:
   `mix test test/canary/errors/ingest_test.exs --trace --max-failures 3`.
   For a health state machine bug:
   `mix test test/canary/health/state_machine_test.exs --trace` (the table
   is pure — the test is deterministic and fast, so `StateMachine.transition/4`
   is the first place to bisect).
   For a webhook signature bug:
   `mix test test/canary/alerter/signer_test.exs --trace`.
3. **Run experiment** — observe result.
4. **Evaluate**:
   - **Disproved** → eliminate this cause, form NEW hypothesis. Ruling
     things out is progress.
   - **Supported** → design next experiment to increase confidence. Not
     proven until you can explain the full causal chain from input to
     observed output.
   - **Ambiguous** → experiment was too broad. Narrow to a single module.
5. **Gate check before claiming done** — `./bin/validate --fast` for a
   quick local sanity loop. Full `./bin/validate --strict` is the
   pre-push gate (what `.githooks/pre-push` runs) and is non-negotiable
   before the fix commit leaves your machine.

Never skip justification. "Just try X" is a red flag.

### Phase 4: Implementation

1. **Write failing test first** — reproduce the bug in a test under the
   appropriate `test/canary/<area>/` directory. Hot-module tests live under
   `test/canary/errors/`, `test/canary/health/`, `test/canary/webhooks/`,
   `test/canary/alerter/`.
2. **Verify test fails for the right reason** — not a compile error, not a
   missing fixture.
3. **Implement single fix** — address root cause. ONE change at a time.
   Do not also refactor the surrounding module "while we're here."
4. **Verify** — `mix test test/canary/<area>/<area>_test.exs --trace`
   passes; coverage on `Canary.Repo`-backed paths still ≥ **81%**;
   `canary_sdk/` coverage still ≥ **90%** if your change touches the SDK.
5. **Run the gate** — `./bin/validate --strict` before push. This is the
   same Dagger entrypoint that hosted CI runs against the trusted snapshot
   at `.ci/trusted/` (see `docs/ci-control-plane.md`); a green local strict
   run is the contract for a green CI run.
6. **Commit with scoped prefix** — `fix(health):`, `fix(ingest):`,
   `fix(webhooks):`, `fix(ci):`, `fix(query):` etc. The PR title carries
   the same prefix — that's what becomes the squash commit on master.
7. **If 3+ fixes failed** — STOP. Question the architecture. See
   `references/systematic-debugging.md`.

## Root Cause Discipline

For each hypothesis, categorize:
- **ROOT**: Fixing this removes the fundamental cause. E.g. "the changeset
  casts `:id`, drop it from the cast list."
- **SYMPTOM**: Fixing this masks an underlying issue. E.g. "retry the insert
  until a non-nil id survives."

Post-fix question: "If we revert in 6 months, does the problem return?"

## Demand Observable Proof

Before declaring "fixed", show:
- A timeline event in the `errors`, `incidents`, or `timeline_events` table
  confirming the expected state transition (query via the read-only API or
  `sqlite3 /data/canary.db`).
- A Fly log line from `flyctl logs --app canary-obs` proving the fix path
  executed.
- A passing narrow `mix test` invocation for the failing test that now
  succeeds.
- For webhook fixes: a delivery row in the ledger showing non-5xx status
  and, if relevant, circuit breaker cleared for that `WHK-…` id.

Mark as **UNVERIFIED** until observables confirm.

## Classification

| Type | Signals | Approach |
|------|---------|----------|
| Test failure (core) | `mix test` assertion in `test/canary/...` | Read test, trace expectation, re-run with `--trace --max-failures 3` |
| Test failure (SDK) | `mix test` assertion in `canary_sdk/` | Same, but run from `canary_sdk/`; SDK coverage gate is 90% |
| Runtime error | BEAM stack trace on `canary-obs` | Stack trace → module path → state via read-only API |
| Changeset validation | Ecto changeset error with missing `id` | Rule #1: struct-literal PK, not cast |
| Oban crash | `oban_jobs` table missing / enqueue fails | Rule #2: run the dedicated migration |
| HTTP client crash | `Req` / `Finch` `ArgumentError` | Rule #3: drop `:connect_options`, use `:receive_timeout` |
| Migration failure | `mix ecto.migrate` references `priv/read_repo/` | Rule #4: remove `ReadRepo` from `ecto_repos` |
| Health checker silent | No target state transitions, no `health.*` timeline events | Check `lib/canary/health/manager.ex` boot log for the `rescue` retry; verify `Canary.Health.StateMachine.transition/4` table |
| Webhook delivery failing | `canary-obs` logs show repeated 5xx / timeouts | Check circuit breaker ETS and `lib/canary/webhooks/delivery.ex`; restart `canary-obs` to reset ETS |
| Fly random port | Fly health check failing; machine bound to non-8080 | Rule #6: explicit `port:` in `runtime.exs` |
| Gate failure | `./bin/validate --strict` red | See `docs/ci-control-plane.md`; reproduce locally before debugging CI |
| DR / backup issue | Litestream silent, restore stale | `docs/backup-restore-dr.md`; `bin/dr-status`; `bin/dr-restore-check` |
| Auth / key rotation | 401s from otherwise valid clients | `docs/api-key-rotation.md` |
| Production incident | `canary-obs` down or ingest-spike alerts | Create `INCIDENT-{timestamp}.md`; follow triage playbook |

## Investigation Work Log (Production Issues)

For non-trivial `canary-obs` issues, create `INCIDENT-{timestamp}.md`:
- **Timeline**: What happened when (UTC). Use `flyctl logs` timestamps.
- **Evidence**: Fly log excerpts, `sqlite3` queries against `/data/canary.db`,
  read-only API queries with the `read-only` scope key.
- **Hypotheses**: Ranked by likelihood, each tagged to a footgun rule or
  module path.
- **Actions**: Commands run, tests executed, machines restarted.
- **Root cause**: When found; cite the specific module/migration file.
- **Fix**: Commit SHA with scoped prefix; `./bin/validate --strict` pass
  receipt.

## Runbook Pointers

- **Data loss, corruption, or DB reset:** `docs/backup-restore-dr.md`.
  The only tested recovery path. Never improvise SQLite deletion on a
  running machine.
- **CI red, gate failing, or Dagger invocation mismatch:**
  `docs/ci-control-plane.md`. Local `./bin/validate --strict` is the
  same entrypoint hosted CI runs; reproduce locally before blaming the
  runner.
- **401 / 403 from otherwise valid API clients, bootstrap key lost,
  scope confusion:** `docs/api-key-rotation.md`.
- **Non-HTTP health checks (desktop apps, cron, workers) behaving
  oddly:** `docs/non-http-health-semantics.md`.
- **Dogfooding / self-monitoring regressions:**
  `docs/networked-service-dogfooding.md` and `bin/dogfood-audit`.

## Red Flags — STOP and Return to Phase 1

- "Quick fix for now, investigate later"
- "Just try changing X and see"
- Multiple simultaneous changes
- Proposing solutions before ruling out the seven footguns
- "One more fix attempt" (when 2+ already tried)
- Each fix reveals new problem in a different module
- Reaching for `Repo.query!` inside a GenServer to work around a missing
  table (this is how rule #2 bugs are born)
- Adding a workaround instead of running the documented DR sequence in
  `docs/backup-restore-dr.md`

## Toolkit

- **Self-observability**: Canary ingests its own errors via
  `Canary.ErrorReporter` (direct ingest, no HTTP loopback). Query its
  own `errors`, `error_groups`, `incidents`, `timeline_events` tables
  through the read-only API to see prior occurrences.
- **Fly logs**: `flyctl logs --app canary-obs`.
- **Fly SSH**: `flyctl ssh console --app canary-obs` for
  `sqlite3 /data/canary.db` inspection.
- **Git**: `git log --grep 'fix('`, `git log --oneline -20 <path>`,
  `git blame`, `git bisect`.
- **Gate**: `./bin/validate --fast` (pre-commit), `./bin/validate --strict`
  (pre-push, full Dagger lanes + advisories), `./bin/validate --advisories`
  (live advisory scan only).
- **Sub-agents**: Parallel hypothesis investigation (see above).
- **/research thinktank**: Multi-model hypothesis validation for genuinely
  ambiguous cases.

## Output

- **Root cause**: What's actually wrong, named by module/migration path or
  footgun rule.
- **Fix**: Commit SHA with scoped prefix (`fix(<area>):`).
- **Verification**: Observable proof — test output, log line, DB query,
  `./bin/validate --strict` pass.

## Gotchas

- **Fixing before investigating:** The #1 failure mode. If you haven't
  ruled out the seven footguns, you don't know the root cause yet.
- **Casting a custom PK:** A changeset that lists `:id` in `cast/3` will
  silently drop it. Always `%Error{id: id} |> Error.changeset(attrs)`.
- **Creating Oban tables from a GenServer:** `Repo.query!` races
  `pool_size: 1`. Only the dedicated Ecto migration
  `priv/repo/migrations/20260314230000_create_oban_jobs.exs` is safe.
- **Passing `:finch` and `:connect_options` to `Req.request/1`:** pick one;
  for Canary, keep `:finch` and express timeouts as `:receive_timeout`.
- **Adding `Canary.ReadRepo` to `ecto_repos`:** breaks `mix ecto.migrate`.
  `ReadRepo` is deliberately absent.
- **Treating the `Health.Manager` boot-retry warning as a bug:** it isn't;
  the `rescue` clause is intentional.
- **Deleting `/data/canary.db` on a running machine:** SQLite WAL keeps the
  handle open; it's a no-op. Follow `docs/backup-restore-dr.md`.
- **Restarting to "clear" the circuit breaker without fixing the receiver:**
  restart is legitimate after the downstream receiver is fixed, not before.
  The ETS reset without a real fix just buys you another 10 failures.
- **Running bare `mix test` as first diagnostic:** use the narrow form:
  `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3`.
- **Config is almost always the answer:** Fly secrets, `runtime.exs`,
  `litestream.yml`. Check config before reading hot-module code.
