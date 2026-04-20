---
name: deliver
description: |
  Inner-loop composer. Takes one backlog item to merge-ready code. Composes
  /shape → /implement → {/code-review + /ci + /refactor + /qa} (clean loop)
  and stops. Does not push, does not merge, does not deploy. Communicates
  with callers via exit code + receipt.json — no stdout parsing.
  Every run also ends with a tight operator-facing brief plus a full
  /reflect session.
  Use when: building a shaped ticket, "deliver this", "make it merge-ready",
  driving one backlog item through review + CI + QA.
  Trigger: /deliver.
argument-hint: "[backlog-item|issue-id] [--resume <ulid>] [--abandon <ulid>] [--state-dir <path>]"
---

# /deliver (canary)

Inner-loop composer. One backlog item from `backlog.d/NNN-*.md` → merge-ready
commits on a feature branch. **Delivered ≠ shipped.** The outer loop
(`/flywheel`) consumes the receipt and decides whether to land and deploy to
`canary-obs`. Humans merge; master keeps linear history with no squash.

## Invariants

- Compose atomic phase skills. Never inline phase logic — especially not
  `/ci`, which owns `./bin/validate --strict` end-to-end.
- Fail loud. A dirty `./bin/validate --strict` lane is dirty — do not
  downgrade it, do not retry past the cap, do not write `merge_ready` when
  Dagger returned non-zero.
- Respect the responder boundary. Do NOT extend `/deliver` to trigger
  webhook replay, repo mutation, or issue creation. Those live in
  downstream responders. Canary owns ingest, health, correlation,
  timelines, queries, and signed webhooks — nothing more.
- No LLM on the request path. If `/shape` or `/implement` proposes a
  request-path model call for summaries, reject the shape and route back.
  Summaries are deterministic templates. This is an invariant, not a style
  preference.

## Input: backlog.d/

`/deliver` reads one item from `backlog.d/NNN-*.md`. The ID is the three-digit
prefix; use `#NNN` when referring to it in prose and commits.

```bash
# Pick explicitly:
/deliver backlog.d/010-ramp-pattern.md
/deliver 020-adminifi-http-surface-verification

# Pick implicitly (no arg):
# read backlog.d/README.md, pick the highest-priority `ready` row.
# `blocked` rows are NEVER picked — they are visible on purpose.
```

Active `blocked` items as of this writing:

| ID  | Title                                | Priority | Est | Blocked on                                                    |
|-----|--------------------------------------|----------|-----|---------------------------------------------------------------|
| #010| Ramp pattern (north star)            | high     | XL  | downstream triage sprite + diff-driven monitor generation     |
| #020| Adminifi HTTP surface verification   | low      | S   | canonical public health URLs for `adminifi-web`/`consumer-portal` |

If `/shape` would require unblocking one of those dependencies first, stop
and route to `/groom` — do not try to force a blocked item through.

## Outer Loop Position

`/deliver` is the inner loop. The canary-ratified outer-loop composition is:

```
/settle → /refactor → /code-review → merge
```

`/deliver` produces the merge-ready artifact that `/settle` then lands.
`/deliver` itself stops at `merge_ready`. It does not run `/settle`, does not
call `gh pr merge`, does not trigger `flyctl deploy --app canary-obs`.

## Composition (inner loop)

```
/deliver [backlog-item|--resume <ulid>]
    │
    ▼
  pick from backlog.d/NNN-*.md  (skip `blocked` rows per README.md)
    │
    ▼
  /shape       → context packet (goal, oracle, sequence, impacted hot modules)
    │
    ▼
  /implement   → TDD on feature branch; commits use conventional-scope prefixes
    │
    ▼
┌── CLEAN LOOP (max 3 iterations) ────────────────────────────────────┐
│  /code-review → critic + philosophy bench, synthesized findings      │
│  /ci          → ./bin/validate --strict via dagger call strict       │
│  /refactor    → diff-aware simplification of impacted hot modules    │
│  /qa          → API-level exercises when a user-facing surface moved │
│  capture evidence under .spellbook/deliver/<ulid>/                   │
└──────────────────────────────────────────────────────────────────────┘
    │ all green → merge_ready (exit 0)
    │ cap hit or hard fail → fail loud (exit 20/10)
    ▼
  receipt.json written; stop. No push, no merge, no deploy.
```

## The Gate: `./bin/validate --strict`

`/ci` inside the clean loop runs the **exact command a human operator runs
before pushing**:

```bash
./bin/validate --strict   # wraps `dagger call strict`
```

This is the local equivalent of hosted `.github/workflows/ci.yml`, which
invokes `dagger call strict --source=../candidate` from the trusted control
plane at `.ci/trusted/`. Local `--strict` and hosted CI share the Dagger
entrypoint, so a green local strict is a high-confidence predictor of a
green merge check.

Deterministic package gates that must all be green for `merge_ready`:

- Core (`lib/canary/**`): compile, format, credo, sobelow, coverage **81%**, dialyzer.
- `canary_sdk/`: compile, format, coverage **90%**.
- TS SDK (`clients/typescript/`): typecheck, coverage, build.
- Strict-only: live dependency advisories, `.codex/agents/*.toml` role validation, git-history secrets scan.

Related flags `/ci` may invoke when narrowing:

- `./bin/validate --fast` — pre-commit parity (`dagger call fast`): lint + core tests only.
- `./bin/validate --advisories` — live advisory scan in isolation.

Never invent parallel vocabulary. Do not tell a caller to "run the tests"
or "run CI" — cite the flag.

## Phase Routing

| Phase     | Skill          | Owns                                                                                      | Skip when |
|-----------|----------------|-------------------------------------------------------------------------------------------|-----------|
| shape     | `/shape`       | context packet, oracle, which hot modules (`lib/canary/...`) the change touches           | packet already has executable oracle |
| implement | `/implement`   | TDD against `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3`; feature branch only | — |
| review    | `/code-review` | critic + philosophy bench, invariant-check (pool_size:1, deterministic summaries, RFC 9457, scoped keys) | — |
| ci        | `/ci`          | `./bin/validate --strict` — coverage 81%/90%, credo, sobelow, dialyzer, advisories        | `/ci` decides — do not pre-filter |
| refactor  | `/refactor`    | diff-aware simplification; prefer deletion in `lib/canary/query.ex`, `lib/canary/incidents.ex`, `lib/canary/errors/ingest.ex` | trivial diffs (<20 LOC, single file) |
| qa        | `/qa`          | API-level exercise against the public contract (router pipelines + `GET /api/v1/openapi.json`) | pure refactor with no public surface change |

Each phase skill has its own receipt. `/deliver` reads them; it never
re-implements the phase.

## Commit Style (load-bearing)

Every implement/refactor commit uses conventional-with-scope:

```
feat(health): add non-http check-in monitors           # #021
refactor(query): split Canary.Query into domain read models (#125)   # #006
fix(ci): make github control plane immutable          # #016
docs(ops): choose non-http health model               # #009
chore(governance): add CODEOWNERS, secret-material gitignore, settings doc (#128)
build: pin local dagger and harden repo validation (#127)
```

Rules:

- Scope reflects the primary system touched: `health`, `query`, `ingest`,
  `webhooks`, `ci`, `ops`, `dr`, `auth`, `governance`, `onboarding`.
- Reference the backlog item ID in the commit body or subject where the
  diff maps one-to-one, e.g. `#021` for `feat(health): add non-http
  check-in monitors` (see `git log`).
- Linear history; no squash on master. Land commits as they were authored.
  `/deliver` emits the commits; `/settle` lands them without flattening.

## Archival on Completion

When `/deliver` exits 0 and `/settle` lands the branch, the item's backlog
file moves to `_done/`. `/deliver` itself does not run the `git mv` — that's
`/reflect` / `/settle`'s close-out — but the shape is load-bearing and
`/deliver` should not write anything under `_done/`:

```bash
git mv backlog.d/021-check-in-monitors-for-non-http-runtimes.md backlog.d/_done/
# commit: chore(backlog): archive #021
```

Recently shipped items follow this exact pattern:

- `backlog.d/_done/021-check-in-monitors-for-non-http-runtimes.md` — #021 (`feat(health):`)
- `backlog.d/_done/016-immutable-ci-control-plane.md` — #016 (`fix(ci):`)
- `backlog.d/_done/006-split-query-read-models.md` — #006 (`refactor(query):`, PR #125)

`backlog.d/README.md`'s priority table must flip the row to `done` in the
same commit. If it doesn't, `/reflect` will flag the drift.

## Cross-Cutting Invariants

- **Feature branch only.** Never commit to master. Linear history is
  preserved by authored commits, not by squashing at merge time.
- **Never push.** `/deliver` stops at merge-ready. `/settle` lands.
- **Never merge.** Humans merge. CODEOWNERS routes review to `@phrazzld`.
- **Never deploy.** `flyctl deploy --app canary-obs --remote-only` is fired
  by `.github/workflows/deploy.yml` after `ci.yml` green on master, or by
  the outer loop. Not by `/deliver`.
- **Never re-deliver stale backlog.** If the item already lives under
  `backlog.d/_done/`, or the branch history contains the item ID in a
  landed commit, stop and route to `/groom tidy`.
- **Evidence is out-of-band.** `/deliver` writes zero artifacts beyond
  receipt + state under `.spellbook/deliver/<ulid>/` (gitignored).
  Per-phase skills emit their own evidence; `/deliver` records pointers.

## Contract (exit code + receipt)

`/deliver` communicates exclusively via its exit code and
`<state-dir>/receipt.json`. Callers — human or `/flywheel` outer loop —
do not parse stdout.

| Exit | Meaning                                           | Receipt `status`         |
|------|---------------------------------------------------|--------------------------|
| 0    | merge-ready                                       | `merge_ready`            |
| 10   | phase handler hard-failed (tool/infra)            | `phase_failed`           |
| 20   | clean loop exhausted (3 iterations, still dirty)  | `clean_loop_exhausted`   |
| 30   | user/SIGINT abort                                 | `aborted`                |
| 40   | invalid args / missing dep skill                  | `phase_failed`           |
| 41   | double-invoke on an already-delivered item        | `phase_failed`           |

The receipt always contains:

- `backlog_item_id` (e.g. `"021"`) and `backlog_item_path` (e.g.
  `"backlog.d/_done/021-check-in-monitors-for-non-http-runtimes.md"` once
  archived).
- `branch` (feature branch name).
- `ci.command` = `"./bin/validate --strict"` and `ci.dagger_call` = `"strict"`.
- `coverage.core` (must be ≥ 0.81 for `merge_ready`) and
  `coverage.canary_sdk` (must be ≥ 0.90).
- `pr_url` if `/settle` has already opened one; otherwise `null`.
- Pointers to per-phase receipts under `.spellbook/deliver/<ulid>/`.

Full schema: `references/receipt.md`.

## Resume & Durability

State is filesystem-backed under `<worktree-root>/.spellbook/deliver/<ulid>/`
(gitignored). `--state-dir <path>` lets `/flywheel` anchor state under its
cycle's evidence tree. After every phase, `state.json` is rewritten
atomically (write → fsync → rename). `--resume <ulid>` reloads and continues;
`--abandon <ulid>` removes state-dir but leaves the feature branch alone.

## Gotchas (judgment, not procedure)

- **Retry vs escalate.** Dirty on iteration 1 → retry. Dirty on iteration 3
  → exit 20, write receipt, hand to operator. Do not invent a 4th iteration.
- **What counts as "dirty".** `/code-review` blocking verdict; `/ci`
  non-zero (any `./bin/validate --strict` lane red); `/qa` P0/P1 finding.
  Coverage regressions below 81% core or 90% `canary_sdk` are dirty.
  P2 QA findings and "nit"/"consider" review notes are NOT blocking — log
  them in the receipt.
- **Invariant violations surface in review, not CI.** Dialyzer will not
  catch "LLM call added on the request path" or "custom PK cast instead of
  struct-set." `/code-review` must check against `CLAUDE.md` footguns —
  especially the six-bug-initial Ecto PK rule and the single-writer
  `Canary.Repo` pool.
- **Ecto PK trap in `/implement`.** If `/implement` introduces a new
  resource with a nanoid PK (`ERR-`, `INC-`, `WHK-`), the struct-set
  pattern is mandatory: `%Error{id: id} |> changeset(attrs)`. Casting the
  `id` silently drops it. `/code-review` treats a casted custom PK as
  blocking.
- **Oban Lite migration.** Any work that touches Oban tables must go
  through a dedicated Ecto migration at
  `priv/repo/migrations/*_create_oban_jobs.exs`, not a GenServer or Release
  module. `Canary.Repo`'s `pool_size: 1` races with anything else.
- **Dagger drift.** If `/ci` reports "local dagger version drift," the fix
  is in `bin/dagger` / `dagger.json`, not a `--no-verify` push. `/deliver`
  never bypasses hooks.
- **Silent push.** A phase skill that "helpfully" runs `git push` is a bug
  in that phase skill. Surface it; do not suppress it.
- **Re-shaping mid-delivery.** If `/implement` or `/qa` reveals the shape
  is wrong (e.g. the item requires work outside the responder boundary),
  stop the clean loop and exit with `remaining_work` pointing at re-shape.
- **Blocked item drift.** `backlog.d/010-ramp-pattern.md` and
  `backlog.d/020-adminifi-http-surface-verification.md` are visible on
  purpose. If a human asks you to deliver one without unblocking its
  dependency, refuse and route to `/groom` or `/office-hours`.

## Closeout Contract

Every `/deliver` run ends with two operator-facing outputs, in order:

1. A tight delivery brief (1-2 short paragraphs or 4-6 flat bullets).
2. A full `/reflect` session.

`receipt.json` is the machine contract and remains authoritative for
`/flywheel` and `/settle`. The brief is for the human operator and must
answer:

- Which backlog item (`#NNN` + title) was delivered and what changed.
- Why making this merge-ready matters now — what Canary capability or
  invariant it strengthens.
- What alternatives the shape considered and why the implemented design
  wins under the current constraints (or why it was the right delivery
  compromise if not clearly best).
- What it creates for agent consumers of the webhooks/query APIs and for
  the operator working `flyctl logs --app canary-obs`.
- What `./bin/validate --strict` verified (which gates ran green,
  coverage numbers) and what residual risk remains before `/settle`
  lands it.

`/reflect` is mandatory and captures learnings, harness changes, and
follow-on backlog mutations (including `git mv backlog.d/NNN-*.md
backlog.d/_done/` when appropriate). Do not collapse it into the brief.

## References

- `references/clean-loop.md` — iteration cap, per-phase dirty detection, escalation
- `references/receipt.md` — full JSON schema, exit-code table, state lifecycle
- `references/durability.md` — atomic checkpoint protocol, `--resume`/`--abandon`
- `references/evidence.md` — per-phase emission paths under `.spellbook/deliver/`
- `references/branch.md` — feature-branch naming, no-push rule
- `references/worktree.md` — state-root resolution for concurrent worktrees
- `backlog.d/README.md` — priority map, dependency graph, Lanes 1–5, status legend
- `docs/ci-control-plane.md` — canonical description of the Dagger/CI handshake

## Non-Goals

- Deploying to `canary-obs` — outer loop / `.github/workflows/deploy.yml` concern.
- Merging — humans merge; CODEOWNERS routes review.
- Pushing — `/settle` pushes once the branch is merge-ready.
- Running Fly operations (`flyctl deploy`, `flyctl ssh`, `flyctl storage create`) — not in scope.
- Rotating API keys or running `bin/dr-restore-check` / `bin/dogfood-audit` — those are operator runbooks (`docs/api-key-rotation.md`, `docs/backup-restore-dr.md`, `docs/networked-service-dogfooding.md`), not `/deliver` concerns.
- Multi-ticket operation — one `backlog.d/NNN-*.md` per invocation.
- Cross-boundary work — repo mutation, issue creation, LLM triage live in downstream responders (e.g. bitterblossom). `/deliver` refuses shapes that cross that line.

## Related

- Consumer: `/flywheel` — outer loop passes `--state-dir` under its cycle tree and reads `receipt.json`.
- Lander: `/settle` — takes a `merge_ready` receipt, runs `./bin/validate --strict` one more time if stale, and lands the branch with linear history. `/deliver` never calls `/settle`.
- Phases: `/shape`, `/implement`, `/code-review`, `/ci`, `/refactor`, `/qa`.
