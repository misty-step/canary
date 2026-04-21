---
name: settle
description: |
  Unblock, polish, and merge. Works in two modes:
  GitHub mode (PR exists): fix CI/conflicts/reviews, polish, refactor, land PR.
  Git-native mode (no PR): use verdict refs, Dagger CI, agent swarm review,
  and land the branch.
  /land alias: validate verdict ref, run Dagger, and land the branch using
  repo policy (default: squash single-ticket branches).
  Use when: PR is blocked, CI red, review comments open, "land this",
  "get this mergeable", "fix and polish", "unblock", "clean up",
  "make this merge-ready", "address reviews", "fix CI", "land this branch".
  Trigger: /settle, /land (alias), /pr-fix, /pr-polish.
argument-hint: "[PR-number|branch-name]"
---

# /settle

Take a canary branch from blocked to clean and (as `/land`) onto `master`
with one squash commit per PR. Plain `/settle` stops at merge-ready. `/land` is the
landing mode of this same skill and continues through `gh pr merge --squash`
per canary's squash-merge policy.

Canary ships on the `/settle → /refactor → /code-review → merge` outer loop.
`/settle` is the unblock + polish + land step. The gate it answers to is
`./bin/validate --strict`, which calls the exact same Dagger entrypoint
(`dagger call strict`) that the hosted immutable control plane
(`.github/workflows/ci.yml`) runs on every PR.

## Role

Senior engineer who owns the lane end-to-end. Not done until the branch is
genuinely clean — not just "`./bin/validate --strict` green" but architecturally
sound, invariant-preserving, and simple. The bar for landing on `canary-obs`
is deterministic summaries, RFC 9457 problem responses, `Canary.Repo` single-writer
discipline, and scoped API key boundaries all intact.

## Execution Stance

You are the executive orchestrator.
- Keep review-comment disposition, invariant-risk tradeoffs, and merge-readiness
  judgment on the lead model.
- Delegate bounded evidence gathering and implementation to focused subagents.
- Use parallel fanout for independent fixes; serialize when fixes share files,
  touch `Canary.Repo` writers, or share Dagger lanes.
- Compose `/ci`, `/code-review`, and `/refactor`; do not replace their domain contracts.

## Mode Detection

`/settle` operates in two modes based on context:

**GitHub mode** — when `$ARGUMENTS` is a PR number, or `gh pr view` succeeds for
the current branch. Uses GitHub PR state, review threads, and `gh` CLI.

**Git-native mode** — when no PR exists. Uses verdict refs (`scripts/lib/verdicts.sh`),
`./bin/validate --strict`, and agent swarm review output. No GitHub API calls.

Detection sequence:
1. If `$ARGUMENTS` matches `^[0-9]+$` → GitHub mode (PR number)
2. If `gh pr view` for current branch succeeds → GitHub mode
3. Otherwise → git-native mode

There is no separate `/land` skill. When invoked as `/land <branch>`, always use
git-native mode regardless of whether a PR exists. `/land` validates the verdict
ref (must exist and point at HEAD), rejects `dont-ship` verdicts, re-runs
`./bin/validate --strict` when available, and lands the branch per canary
policy: **squash-merge**. In GitHub mode use `gh pr merge --squash` with a
pre-composed `--subject`/`--body`; in git-native mode use
`git merge --squash <branch> && git commit` on `master` then `git push`.
`SPELLBOOK_NO_REVIEW=1` bypasses the verdict gate for emergencies.

## Objective

Take the current branch through three phases until it reaches:
- No merge conflicts
- `./bin/validate --strict` green locally (+ the `dagger` check in hosted CI
  when in GitHub mode — same Dagger entrypoint, different checkout shape)
- Every review finding addressed
- Architecture reviewed with the canary-invariants lens
- Tests audited: core coverage at or above 81%, `canary_sdk/` at or above 90%
- Complexity reduced where possible
- Docs current (especially `docs/ci-control-plane.md`, `docs/backup-restore-dr.md`,
  `docs/api-key-rotation.md` when the diff touches their surfaces)

## Executive / Worker Split

Keep synthesis, judgment, and user communication on the lead:
- deciding which review comments are valid, in scope, or rejected
- hindsight architecture review against canary invariants
- confidence assessment and final merge-readiness judgment

Delegate bounded remediation to ad-hoc **general-purpose** subagents:
- fixing one comment thread or one failing Dagger lane at a time
- gathering review evidence, reproducing `./bin/validate --strict` failures,
  and drafting narrow patches
- mechanical cleanups, focused test additions, and doc refreshes with clear ownership

Use **Explore** subagents for evidence gathering when no file mutations are needed.
Use **builder** agent for fixes that require TDD discipline — the canary
default is TDD against `mix test test/canary/<area>/<area>_test.exs --trace
--max-failures 3`, not the full suite.

## Process

### Phase 1: Fix — Unblock

Read `references/pr-fix.md` and follow it completely.

**Goal:** Get from blocked to green on `./bin/validate --strict`.

1. **Conflicts** — rebase or merge, resolve all conflicts. Either works under
   squash-merge (the branch commits don't land on `master`; only the squash
   commit does). `git rebase origin/master` is still convenient for review
   readability.
2. **Gate** — invoke `/ci` for gate ownership, which runs `./bin/validate`
   (default) or `./bin/validate --strict` before landing. In GitHub mode, also
   wait on the `dagger` check in `.github/workflows/ci.yml` — it runs
   `dagger call strict --source=../candidate` from a trusted `.ci/trusted/`
   base snapshot. Do **not** hand-edit `.github/workflows/ci.yml` from a PR
   branch to chase a failure: the workflow is evaluated from the base branch
   per `docs/ci-control-plane.md`, so in-PR edits to it will not affect the
   required check. Fix the Dagger module or the candidate diff instead.
3. **Self-review** — read the entire diff as a reviewer would. Sanity-check
   the canary invariants explicitly: Ecto custom string PKs (`ERR-`, `INC-`,
   `WHK-` nanoids) set on the struct not via `cast`; `Req.request/1` does not
   pass both `:finch` and `:connect_options`; `ReadRepo` is absent from
   `ecto_repos`; no LLM on the request path; RFC 9457 shape on every error
   response.
4. **Review findings** —
   - **GitHub mode:** read every PR comment via
     `skills/settle/scripts/fetch-pr-reviews.sh` (no 300-char previews)
   - **Git-native mode:** run `/code-review` if no verdict ref exists, then read
     `.evidence/<branch>/review-synthesis.md` for findings
   - For each finding: fix (in scope), defer (out of scope → `backlog.d/`), or
     reject (with reasoning referencing the invariant or doctrine it would violate)
5. **Async settlement** —
   - **GitHub mode:** wait for the `dagger` check and bot reviewers; re-check
     via `gh pr view --json statusCheckRollup,reviews`
   - **Git-native mode:** re-run `./bin/validate --strict` after fixes. No
     async bots to wait for.

Dispatch fixes to smaller worker subagents when scope is clear and bounded —
one Dagger lane failure per subagent, one review comment thread per subagent.

6. **Branch-head gate** — canary squash-merges, so only the squash-commit
   state lands on `master`. Run `./bin/validate --strict` once against the
   branch HEAD before merge. No per-commit gate; no bisect-cleanliness
   requirement on the branch commits.
7. **Merge-readiness verification** —
   - **GitHub mode:** `gh pr view --json reviews,statusCheckRollup` — at least
     one approving review, the `dagger` check passing
   - **Git-native mode:** `source scripts/lib/verdicts.sh && verdict_validate <branch>`
     — verdict ref exists and SHA matches HEAD. Plus `./bin/validate --strict`
     is green.

**Exit gate:** `./bin/validate --strict` green, all review findings addressed,
merge-readiness verified, each commit independently clean.

If already green and settled, skip to Phase 2.

### Phase 2: Polish — Elevate quality

Read `references/pr-polish.md` and follow it completely.

**Goal:** Get from "works" to "exemplary." Canary's product north star is
agent-native — bounded, structured, summarized. Polish against that bar.

1. **Hindsight review** — "Would we build it the same way starting over?"
   Read the full diff. Canary-specific checks:
   - Shallow modules or pass-throughs around `lib/canary/errors/ingest.ex`,
     `lib/canary/incidents.ex`, `lib/canary/query.ex`,
     `lib/canary/incident_correlation.ex`, `lib/canary/health/manager.ex`,
     `lib/canary/health/state_machine.ex`, `lib/canary/webhooks/delivery.ex`
   - Hidden coupling or temporal decomposition in `Canary.Health.Manager`
     (the `rescue` in `handle_info(:boot)` is load-bearing, not a code smell)
   - `Canary.Health.StateMachine.transition/4` must remain pure — no side effects
   - Missing or premature abstractions in the new read-model split
     (`lib/canary/query.ex` → domain read models, PR #125)
   - Summary generation that accidentally reaches for an LLM (invariant: deterministic templates)
   - Tests that assert on implementation (Repo internals, Oban table shape)
     instead of behavior (ingest → correlated incident → signed webhook → timeline)
2. **Agent-first assessment** — run `assess-review` (triad, strong tier) for
   structured code review. Run `assess-tests` for test quality scoring. Run
   `assess-docs` if docs were touched (especially `docs/ci-control-plane.md`,
   `docs/backup-restore-dr.md`, `docs/api-key-rotation.md`,
   `docs/networked-service-dogfooding.md`, `docs/non-http-health-semantics.md`,
   `docs/governance.md`). Address all `fail` findings before proceeding.
3. **Architecture edits** — fix what the hindsight review and assess-* checks
   find. Commit with a conventional-with-scope subject
   (`refactor(query):`, `feat(health):`, `fix(ci):`, `chore(governance):`,
   `docs(ops):`, `build:`).
4. **Test audit** — coverage gaps, brittle tests, missing edge cases. Core
   coverage must stay at or above **81%**; `canary_sdk/` at or above **90%**.
   The strict Dagger lane will fail otherwise. Prefer narrow runs:
   `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3`.
5. **Contract check** — if the diff touches router pipelines, controllers,
   or payload shape, re-generate and verify the OpenAPI at
   `GET /api/v1/openapi.json` (source in `priv/openapi/`, embeds the canonical
   replay guide in `info.x-agent-guide`). It is the agent contract; don't
   drift it silently.
6. **Docs** — update any docs/comments that are stale after the changes.
   Webhook payload and event-type changes must reflect in consumer-facing
   docs since responders (e.g. `bitterblossom`) treat them as stable contracts.
7. **Confidence assessment** — how confident are we this won't break anything?
   Treat confidence as an explicit deliverable with evidence: which Dagger
   lanes ran, which narrow `mix test` paths passed, which invariants were
   re-checked.

Use the strongest available model for hindsight review and confidence judgment.
Use smaller workers for narrow polish follow-through once the direction is clear.

**Exit gate:** Architecture clean, invariants preserved, coverage thresholds
met, docs current, confidence stated.

If polish generates changes, return to Phase 1 (`./bin/validate --strict` must stay green).

### Phase 3: Refactor — Reduce complexity

Invoke `/refactor` for this branch and use it as the simplification engine.

**Goal:** Remove complexity that doesn't earn its keep. Code is a liability —
every line on `master` fights for its life.

1. **Run refactor pass** — invoke `/refactor` and rely on its built-in
   base-branch auto-detection; pass `--base master` only if auto-detection
   fails or is ambiguous.
2. **Select one bounded change** — deletion > consolidation > state reduction >
   naming clarity > abstraction. Especially scrutinize: new GenServer boundaries
   around `Canary.Repo` writes (pool_size: 1 means serializing anyway —
   an extra process rarely earns its keep), new indirection layers between
   the router pipelines (`:scope_ingest|:scope_read|:scope_admin`) and their
   controllers, and any shim around the Dagger module.
3. **Implement + verify** — preserve behavior, re-run
   `./bin/validate --strict`, commit with a `refactor(<scope>):` subject.
4. **Validate simplification** — run `assess-simplify` (strong tier) when available.
   `complexity_moved_not_removed` must be false to exit this phase.

**Mandatory when diff >200 LOC net.** For smaller diffs, manual module-depth
review using Ousterhout checks: shallow modules, information leakage,
pass-throughs, compatibility shims with no active contract.

**Exit gate:** No obvious complexity to remove, or explicit justification for keeping it.

If refactor generates changes, return to Phase 1 (`./bin/validate --strict` must stay green).

## Loop Until Done

```text
Phase 1 (fix) → Phase 2 (polish) → Phase 3 (refactor)
       ↑                                      │
       └──────── if changes pushed ───────────┘
```

Each phase that generates commits sends you back to Phase 1 to re-verify
`./bin/validate --strict` and reviews. The loop terminates when a full pass
produces no changes. The outer loop above this skill is
`/settle → /refactor → /code-review → merge` — this skill runs the
`/settle` leg and, as `/land`, closes it.

## Landing (`/land` mode)

When invoked as `/land`, after all three phases are green:

1. Re-confirm `./bin/validate --strict` on the branch HEAD.
2. Fetch and rebase onto `origin/master` (optional — conflicts resolved
   pre-squash make the PR diff clean for reviewers).
3. Land per canary policy — **squash-merge**:
   - GitHub: `gh pr merge <PR> --squash --subject "<conventional-with-scope subject>" --body "<summary>" --delete-branch`
   - Git-native: `git checkout master && git merge --squash <branch> && git commit -m "<conventional-with-scope subject>" && git push origin master`
4. The PR title / squash subject carries the conventional-with-scope prefix
   (`feat(health):`, `fix(ci):`, etc.); branch commits stay for review
   readability but don't land on `master`.
5. Verify `git log --oneline -3 origin/master` shows the squash commit as
   a single entry.

## Reviewer Artifact Policy

When settlement needs screenshots, videos, logs, or walkthrough proof:

**GitHub mode:**
- Upload screenshots/GIFs to draft GitHub release assets, embed download URLs
  in PR comments. See `skills/demo/references/pr-evidence-upload.md` for the recipe.
- Convert `.webm` → `.gif` before upload (GitHub renders GIFs inline, not video).
- Never use `raw.githubusercontent.com` URLs (breaks for private repos).
- Link the full release at the bottom of every evidence comment.

**Git-native mode:**
- Store evidence in `.evidence/<branch>/<date>/` (LFS-tracked for binaries).
- Write `qa-report.md` and `review-synthesis.md` alongside captures.
- Commit evidence to the branch. It becomes part of the auditable history.
- Convert `.webm` → `.gif` before committing (easier to browse).

**Both modes:**
- Prefer Dagger-lane artifacts or GitHub step summaries for generated verification output.
- Never commit binary evidence directly to tracked git (use LFS or GitHub releases).

## Flags

- `$ARGUMENTS` as PR number — target specific PR
- If no argument, uses the current branch's PR

## Anti-Patterns

- Declaring "done" while the `dagger` check is still running in hosted CI.
- Ignoring review comments instead of addressing them.
- **Truncating review comments** — reading 300-char previews instead of full
  text. Run `skills/settle/scripts/fetch-pr-reviews.sh <PR>` to get complete bodies.
- **Reflexive dismissal** — rejecting automated reviewer comments with "by
  design" or "established pattern" without steelmanning the argument. See
  disposition criteria in `references/pr-fix.md`.
- **Batch reply without fixing** — replying to all comments in one PR comment
  instead of addressing each inline, one at a time.
- **Editing `.github/workflows/ci.yml` from a PR branch to escape a red gate.**
  The control plane is immutable by design (`docs/ci-control-plane.md`): the
  workflow runs from the base snapshot in `.ci/trusted/`, so the edit in your
  PR does nothing for the required check. Fix the Dagger module or the code.
- **Using `--merge` or `--rebase`.** Canary lands one squash commit per PR.
  `gh pr merge --merge` fills `master` with branch commits; `--rebase`
  replays them. Neither matches policy.
- **Squash subject without conventional-with-scope prefix.** The squash
  commit IS the master history; it must carry `feat(health):` /
  `fix(ci):` / etc. for `git log` readability and downstream tooling.
- **Forgetting `--delete-branch` on the squash merge.** Branches left
  dangling on the remote after a squash land are clutter.
- **Relying on bisect-cleanliness of branch commits.** Branch commits
  don't reach `master` under squash-merge; only the squash commit does.
  Run `./bin/validate --strict` once on branch HEAD, not per commit.
- Polish without re-running `./bin/validate --strict` afterward.
- Refactoring without verifying invariants are preserved (pure
  `StateMachine.transition/4`, deterministic summaries, RFC 9457 shape,
  single-writer `Canary.Repo`).
- Skipping refactor because "it works."
- Posting "PR Unblocked" while async reviewers can still add findings.
- Merging from plain `/settle` — `/settle` ends at merge-ready. Use `/land`
  when the task is to land the branch.
- **Git-native mode: merging without a verdict ref.** Always validate via
  `verdict_validate` before merging. No verdict = no merge.

## Output

Receipt per run:
- **Fix:** conflicts resolved, Dagger lanes fixed, review comments addressed
  (count + dispositions), per-commit gate status.
- **Polish:** architecture changes made, test gaps filled (coverage deltas
  vs. 81% / 90%), invariants re-verified, confidence level + evidence.
- **Refactor:** complexity removed (LOC delta, modules consolidated,
  abstractions deleted).
- **Final receipt:**
  - PR URL (GitHub mode) or branch + verdict SHA (git-native mode)
  - Resulting `master` SHA after land (if `/land` ran)
  - One-line summary of changes
  - Whether `./bin/validate --strict` was last-run-green and on which SHA
  - Any remaining risks or backlog items filed under `backlog.d/`
