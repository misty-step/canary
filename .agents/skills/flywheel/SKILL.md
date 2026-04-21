---
name: flywheel
description: |
  Outer-loop shipping orchestrator. Composes /deliver, landing, /deploy,
  /monitor, and /reflect cycle per backlog item, then applies reflect
  outputs to the harness and backlog before looping.
  Use when: "flywheel", "run the outer loop", "next N items",
  "overnight queue", "cycle".
  Trigger: /flywheel.
argument-hint: "[--max-cycles N]"
---

# /flywheel

Compose cycles of: pick one `backlog.d/NNN-*.md` → `/deliver` → `/settle`
(land) → `/deploy` (Fly) → `/monitor` → `/reflect cycle` → apply reflect's
outputs → loop.

You already know how to do each of these. This skill exists only to encode
the canary-specific invariants that aren't inferable from the leaf names.

## Per-cycle shape

1. **Select.** Read `backlog.d/README.md` priority map. Pick the highest-
   priority `ready` item. Optional lane filter (e.g. `--lane 4` for
   overnight hardening runs — see Lanes below). Both currently active
   items are `blocked`: `#010 Ramp pattern` (waits on bitterblossom
   `bb/011` triage sprite) and `#020 Adminifi HTTP surface verification`
   (waits on upstream adminifi stability). If nothing is `ready`, halt
   and surface the blocker chain from the dependency map.
2. **Deliver.** `/deliver <item>` drives `/shape → /implement →
   {/code-review + /ci + /refactor + /qa}` on a feature branch until
   `merge_ready`. Inner gate is `./bin/validate --fast` (wired via
   `.githooks/pre-commit`, runs `dagger call fast`). Contract is exit
   code + `receipt.json` — do not peer inside.
3. **Land.** `/settle` is the canary-ratified merge step. It runs
   `./bin/validate --strict` (full `dagger call strict` gate: 81% core
   coverage, 90% `canary_sdk`, format/credo/sobelow/dialyzer, live
   advisories, `.codex/agents/*.toml` role validation), then lands via
   `gh pr merge --squash --delete-branch` with a pre-composed subject
   carrying the conventional-with-scope prefix (e.g. `feat(health):`,
   `fix(ci):`, `refactor(query):`), then archives:
   `git mv backlog.d/NNN-item.md backlog.d/_done/`.
4. **Deploy.** Hosted CI auto-deploys via `.github/workflows/deploy.yml`
   after `ci.yml` green — that workflow fires `flyctl deploy --app
   canary-obs --remote-only`. If the dispatch didn't fire (workflow
   disabled, manual cycle, out-of-band hotfix), `/deploy` handles the
   Fly dispatch directly against `canary-obs`.
5. **Monitor.** `/monitor` polls `https://canary-obs.fly.dev/healthz`
   and `/readyz` through a grace window. Canary also self-reports via
   `Canary.ErrorReporter` (`:logger` handler → direct-ingest, no HTTP) —
   treat any fresh error group on the deployed SHA as a trip signal.
6. **Reflect.** `/reflect cycle <cycle-ulid>` emits session retro +
   harness-tuning suggestions to a branch. Mutations to `backlog.d/` and
   memory land before the cycle closes. Harness edits never touch
   `master`.

## Outer-loop composition (ratified)

Inside `/deliver`'s review sub-loop the shape is **`/settle → /refactor →
/code-review → merge`**. Do not reorder. `/settle` unblocks first, then
refactor shrinks the diff, then `/code-review` runs against the polished
diff, then merge. Flipping `/refactor` and `/code-review` causes re-review
churn on reviewer-owned fix pins.

## Lanes (from `backlog.d/README.md`)

- **Lane 1 — agent readiness.** `#010 ramp pattern` north star. Depends
  on `#012` (delivery ledger, done) + `bb/011` (bitterblossom triage
  sprite, external).
- **Lane 2 — contract + observability.** `#011 OpenAPI` and `#013
  metrics` (both done). Parallelizable.
- **Lane 3 — structural.** `#006 query split` (done) → `#005 connect-a-
  service` (done).
- **Lane 4 — hardening.** `#008 #014 #016 #017 #018 #019` family — CI,
  governance, DR, Dagger. Small, independent — overnight-queue friendly.
- **Lane 5 — future.** `#020 Adminifi HTTP surface verification`
  (blocked on upstream).

`/flywheel --lane 4` restricts selection to that lane for targeted runs.

## Invariants

- Flywheel composes. Phase logic lives in the leaf skill.
- State lives in `/deliver`'s `receipt.json`, git, and `backlog.d/`.
  Flywheel has none.
- Land before deploy. Always. `master` must be green before the auto-
  deploy workflow fires.
- `./bin/validate --strict` is the convergence gate — never
  `mix test` bare, never `dagger call` bare, never "run CI".
- Archival move (`git mv … backlog.d/_done/`) happens in the landing
  commit, not after. A shipped item left in `backlog.d/` is a lie the
  next cycle will trip on.
- `Canary.Repo` pool_size:1 holds across the cycle — do not schedule
  two flywheel runs against the same worktree.

## Stop conditions

- **No `ready` items.** Halt. Report the blocker chain from the
  dependency map (e.g. "`#010` blocked on `bb/011` triage sprite;
  `#020` blocked on upstream adminifi").
- **`./bin/validate --strict` fails twice for the same reason.** Halt
  and hand off to `/diagnose`. Do not paper over with `--no-verify` or
  by dropping a coverage threshold — those are load-bearing walls.
- **Canary self-reports an error in the monitor window.** The
  `ErrorReporter` direct-ingest caught a regression on the just-
  deployed SHA. Halt, hand off to `/diagnose`, consider rollback via
  the previous `flyctl releases` entry.
- **`/reflect` emits a blocking harness-tuning action.** Halt, apply
  the fix on the harness branch, then re-enter the loop.

## Gotchas

- `/deliver`'s `receipt.json` is the contract — don't peer inside, don't
  regex its prose.
- A `backlog.d/` item can be open but already shipped in git (archive
  drift from a crashed prior cycle). Fix the stale entry by moving to
  `_done/` before starting a fresh cycle on it.
- Two `/flywheel` runs in the same worktree collide on git state and
  the SQLite single-writer pool. Use separate worktrees for parallelism.
- Deploy is a no-op for repos inside the monorepo that don't ship to
  Fly (`canary_sdk/`, `clients/typescript/`, `dagger/`) — land + reflect
  still run. Auto-deploy only fires when core paths change.
- SQLite WAL on `/data/canary.db` means nuclear resets require stop →
  rm → start (per `CLAUDE.md`). Never attempt this mid-cycle.

## Non-goals

- No cycle state machine, event enum, lock, or pick scoring.
- No USD tracking — orchestrator runs under subscription.
- No LLM on the request path at any point in the cycle — summaries
  stay deterministic templates; that's an invariant, not a preference.
- No dashboard-driven steps. Every transition is CLI-first.
