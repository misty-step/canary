---
name: yeet
description: |
  End-to-end "ship it to the remote" in one command. Reads the whole worktree,
  understands what's in flight, tidies debris, splits pending work into
  semantically-meaningful conventional commits, and pushes.
  Not a git wrapper — a judgment layer on top of git. Decides what belongs,
  what doesn't, and how to slice the diff into commits a reviewer can read.
  Use when: "yeet", "yeet this", "commit and push", "ship it", "tidy and
  commit", "wrap this up and push", "get this off my machine".
  Trigger: /yeet, /ship-local (alias).
argument-hint: "[--dry-run] [--single-commit] [--no-push]"
---

# /yeet (canary)

Take the current canary worktree state → one or more conventional commits →
`origin <branch>`. One command. Executive authority. No approval gates.

`/yeet` stops at push. It does NOT open PRs, land branches, merge, or deploy
— branch-landing with review loops is `/settle`; deploy is `/deploy`. This
skill's scope is: worktree → commits → remote.

## Stance

1. **Act, do not propose.** Stage what belongs, leave out what doesn't,
   delete debris, split logically, push. Escalate only on red-flag state
   (see Refuse Conditions).
2. **Clean tree is the deliverable.** `/yeet` is not done while
   `git status --short` still shows modified, staged, or untracked paths.
   Resolve every path by commit, ignore, move out of the repo, or delete.
3. **Reviewability is the product.** A stack of three focused commits beats
   one 2,000-line "wip" commit. Canary squash-merges PRs, so branch commits
   don't land on `master` — but reviewers read them, and one squash commit
   per PR means the PR title carries the master history. Slice accordingly.
4. **Never lose work.** Untracked scratch that might be the user's in-flight
   thinking gets moved (`~/vault/canary/scratch/…`), not deleted, unless it's
   unambiguous debris.
5. **Conventional Commits, always.** Type, scope matching the touched
   subtree, imperative subject. Body explains *why*, cites backlog item IDs
   (`#NNN`) when a commit closes or advances backlog work.
6. **Let the gate run.** `.githooks/pre-commit` runs `./bin/validate --fast`;
   `.githooks/pre-push` runs `./bin/validate --strict`. Never pass
   `--no-verify`. Never bypass. If the gate fails, fix the root cause and
   create a NEW commit — never `--amend` to cover a hook-failed commit.

## Modes

- Default: classify → stage → split into commits → push to `origin <branch>`.
- `--dry-run`: report the plan (commit boundaries, messages, skips), do not
  execute. Skips hooks entirely.
- `--single-commit`: skip the split pass; one commit for everything that
  belongs. Use sparingly — multi-concern single commits are harder to review
  even though they'll squash into one master commit at merge time.
- `--no-push`: commit locally but don't push. `./bin/validate --fast` still
  runs (pre-commit); `--strict` does not (no push).

## Process

### 1. Read the worktree holistically

- `git status --short` (untracked, modified, staged — full picture).
- `git diff --stat` + `git diff --stat --cached` (sizes + files).
- `git log -20 --oneline` (commit-scope conventions for this repo).
- `git rev-parse --abbrev-ref HEAD` (branch, for push target).
- `git log -30 --format='%an %ae' | sort -u` (co-author convention — canary
  today has only `phaedrus`/`phrazzld`; no Co-Authored-By trailer unless
  that changes).
- `git status` for rebase/merge/cherry-pick in progress — refuse if so.

If the tree is clean, say so and exit.

### 2. Classify every file

For each changed / untracked path, assign one of:

| Class | Meaning | Action |
|---|---|---|
| **signal** | Work the user meant to do | Include in a commit |
| **debris** | Unambiguous trash (`.DS_Store`, `_build/`, `deps/`, `cover/`, `.elixir_ls/`, `erl_crash.dump`, `thinktank.log`, `dagger/sdk/` runtime cache, editor swap files) | Delete outright |
| **drift** | Unrelated work from another concern / earlier session | Separate commit (often `chore(governance):` for CLAUDE.md/AGENTS.md edits), move out of repo, or ignore with explicit rationale |
| **evidence** | `/demo`, `/qa`, or DR-drill artifacts | Route to an established artifact path (canary keeps `design-catalog.html` at repo root as the QA-artifact pattern) or move to `~/vault/canary/evidence/`. Do not leave unclassified |
| **scratch** | Half-written notes, TODO files, planning docs | Move to `~/vault/canary/scratch/` or delete if trivial |
| **secret-risk** | Credentials, tokens, live DBs, secret-material dirs | **REFUSE the commit**, surface to user |

**Canary secret-risk paths — always refuse:**

- Anything under `gentle-working-tundra/`, `polished-marching-river/`, or
  `sunlit-moving-walnut/` — these are thinktank secret-material directories
  gitignored in canary. If they appear in the diff, something is
  misconfigured. Refuse.
- `canary.db`, `canary_dev.db`, `canary_test.db`, any `*.db`, `*.db-wal`,
  `*.db-shm` — live SQLite files (gitignored for a reason). If staged,
  refuse.
- `erl_crash.dump` — Erlang crash dumps can contain in-memory secret
  material.
- `.env` or `.env.*` outside `.env.example`.
- `priv/secrets/*` if present.
- `*.pem`, `*.key`, `*.crt`, `*.cer`, `*.der`, `*.p12`, `*.pfx`, `*.jks`,
  `*.keystore`, `id_rsa*`, `id_ed25519*`, `id_ecdsa*`, `credentials*.json`,
  `service-account*.json`, `*.tfvars` (sans `*.tfvars.example`).
- `*.secret` files.
- Fly.io deploy tokens: `FLY_API_TOKEN`, `FLY_AUTH_TOKEN` in diff content.
- Tigris credentials: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`.
- Bootstrap API key material — grep the diff for
  `"Bootstrap API key:"` (logged once on first boot; never commit it).
- Bearer tokens in captured request/response evidence from `/demo` or
  `/qa` runs — force redaction before commit.
- Generic secret regexes in diff content:
  `-----BEGIN.*PRIVATE KEY-----`,
  `api[_-]?key.*=.*["'][^"']{20,}`,
  `(AKIA|ghp_|github_pat_|sk-)[A-Za-z0-9]{16,}`.

**Canary debris heuristics:**

- `_build/`, `deps/`, `cover/`, `.elixir_ls/`, `.mix/` — Elixir/Mix artifacts.
- `node_modules/` — anywhere (root or `clients/typescript/` or `dagger/`).
- `dagger/sdk/` — Dagger SDK runtime cache (the committed module is
  `dagger/src/`, `dagger/scripts/`, `dagger/package.json`; `dagger/sdk/`
  regenerates).
- `.DS_Store`, `Thumbs.db`, `*.swp`, `*.swo`, `*~`, `.#*`.
- `thinktank.log`, any `*.log` at repo root.
- `/tmp/` contents — already gitignored.
- `.agents/`, `.codex/`, `.spellbook/` — harness mirror output, gitignored.

**Drift heuristics:**

- Edits to `CLAUDE.md` footguns or `AGENTS.md` invariants from a feature
  branch that isn't about governance → split to a dedicated
  `chore(governance):` commit. CLAUDE.md is append/merge-only — load-bearing.
- `.claude/` skills/agents edits landing in the middle of a product feature
  → own commit, scoped `chore(governance):` or `chore(harness):`.
- Random edits in an unrelated `lib/canary/…` module with no tests touched
  → ask the user before rolling in; most often it's drift from a prior
  session.

### 3. Group signals into semantic commits

**One concern per commit.** Canary squash-merges PRs, so branch commits
don't appear on `master` — but reviewers read them individually. Each
commit should still pass
`./bin/validate --strict` standalone. For multi-commit branches, verify
per-commit before push:

```bash
git rebase -x './bin/validate --strict' origin/master
```

**Scope selection — match the touched subtree to the scope seen in
`git log`:**

| Touched subtree | Scope |
|---|---|
| `lib/canary/health/*`, `lib/canary_web/controllers/health_*` | `health` |
| `lib/canary/query.ex` + domain read models under `lib/canary/query/` | `query` |
| `lib/canary/errors/ingest.ex`, `lib/canary/errors/*` | `ingest` |
| `lib/canary/webhooks/*`, `lib/canary/alerter/*` | `webhooks` or `alerter` |
| `lib/canary/incidents.ex`, `lib/canary/incident_correlation.ex` | `incidents` |
| `lib/canary/timeline.ex` | `timeline` |
| `lib/canary/auth/*`, scoped API key plumbing | `auth` |
| Onboarding flows, bootstrap docs | `onboarding` |
| DR / Litestream / Tigris recovery | `dr` |
| `.github/workflows/*`, `bin/dagger`, CI plumbing | `ci` |
| `dagger/src/*`, `dagger/scripts/*`, `dagger.json`, `bin/validate` | `build` (or `ci` when hosted-CI wiring is the change) |
| `docs/*` operator runbooks | `ops` |
| `CLAUDE.md`, `AGENTS.md`, `.claude/*`, `CODEOWNERS`, `docs/governance.md` | `governance` |
| `canary_sdk/*` | `sdk` (ships as its own versioned artifact — separate commit from core) |
| `clients/typescript/*` | `ts-sdk` |

Recent canary commits for reference:

```
ec030b9 feat(health): add non-http check-in monitors
2a60dfd feat(ops): codify networked dogfood audit
995c615 fix(ci): make github control plane immutable
34e7ad9 chore(governance): add CODEOWNERS, secret-material gitignore, settings doc (#128)
995ac01 refactor(query): split Canary.Query into domain read models (#125)
bf15708 fix(dr): harden fly tigris recovery
e9b36f5 build: pin local dagger and harden repo validation (#127)
c58d980 docs(ops): choose non-http health model
33bd0a0 feat(auth): add scoped API keys
```

**Grouping heuristics specific to canary:**

- **Ecto migration + schema module + schema tests + `priv/openapi/*` update
  + router pipeline** → often one `feat(<scope>):` commit when they're the
  surface change for one feature. Don't split a half-wired migration from
  its schema.
- **`lib/canary/<area>/*` + `test/canary/<area>/*`** for the same area →
  same commit (co-changed tests belong with their code).
- **`dagger/src/index.ts` + regenerated output from
  `dagger/scripts/sync_source_arguments.py --write`** → same commit to keep
  them in sync, scoped `build:` or `ci:`.
- **Backlog item close-out**: when a commit finishes backlog work, `git mv
  backlog.d/NNN-slug.md backlog.d/_done/NNN-slug.md` in the same commit and
  cite `#NNN` in the body.
- **SDK boundary**: `canary_sdk/` changes live in their own commit even when
  related to core. The SDK ships as its own versioned artifact.
- **TS client boundary**: `clients/typescript/` changes ship in their own
  commit, scoped `feat(ts-sdk):` or similar.
- **Refactors before features.** If the diff mixes a pure refactor under
  `lib/canary/query/` with a new read model that builds on it, refactor
  commits first.

If the user passed `--single-commit`, skip grouping; everything signal-class
becomes one commit. Note the caveat above.

### 4. Write commit messages

Conventional Commits with scope. Format:

```
<type>(<scope>): <imperative subject under 72 chars>

<body: why, not what. Wrap at 72. Cite backlog IDs as #NNN when closing
or advancing a backlog item. Cite PR numbers only when the convention
in git log shows them (canary does both — match the session).>
```

**Types:** `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `build`, `ci`,
`chore`, `style`. No `wip`, no `misc`, no `update`.

**Subject rules:**

- Imperative ("add", not "added" or "adds").
- No trailing period.
- Under 72 chars.

**Body rules:**

- Omit if the subject is self-explanatory.
- Explain the *why* — the constraint, the incident, the reason this was
  the right call over alternatives.
- Reference the load-bearing doc when touching operator-facing
  infrastructure (e.g. `docs/ci-control-plane.md` for CI control-plane
  edits, `docs/backup-restore-dr.md` for DR edits).
- Cite `#NNN` when closing or advancing a backlog item.
- Do NOT restate the file-level diff.

**Co-author.** Canary's `git log` shows only `phaedrus`/`phrazzld`. Do not
append a `Co-Authored-By` trailer unless the user explicitly asks. Re-grep
`git log -30 --format='%an %ae' | sort -u` before deciding; if the
convention has changed, match the new convention.

### 5. Stage, commit, push

- `git add <path>` only the signal paths for each commit — path-by-path,
  never `git add -A` at the root.
- `git commit` per group. `.githooks/pre-commit` will run
  `./bin/validate --fast` (wraps `dagger call fast` — pre-commit lint +
  core tests). Let it run. If it fails:
  1. Read the failure — usually lint, format, or a narrow test.
  2. Fix the root cause.
  3. `git add` the fix.
  4. Create a **new commit** (the hook-failed attempt never landed, so
     nothing to `--amend`). If the failure is in the intended commit's
     content, amend the staged index and retry the commit — but never use
     `--amend` to paper over a previously hook-rejected commit.
- For multi-commit branches, verify per-commit before push:
  `git rebase -x './bin/validate --strict' origin/master`. If a middle
  commit fails strict in isolation, split it, because canary requires
  every commit to stand alone on `master`.
- `git push origin <branch>`. If upstream isn't set, `git push -u origin <branch>`.
  `.githooks/pre-push` runs `./bin/validate --strict` — this is the local
  mirror of the hosted-CI gate (`dagger call strict --source=../candidate`
  from `.ci/trusted/`; see `docs/ci-control-plane.md`). Let it run.
- **If `--strict` fails at push**, stop and escalate to `/diagnose`. Do not
  push broken state. Common strict-only failures: live dependency
  advisories, `.codex/agents/*.toml` role validation, dialyzer, coverage
  under **81%** (core) or **90%** (`canary_sdk`).
- If `git push` is rejected because upstream moved:
  1. `git pull --rebase origin <branch>` (canary is linear — no merge
     commits on branches headed for `master`).
  2. Re-run `./bin/validate --strict` locally if rebase changed anything
     non-trivially.
  3. Retry push once.
  4. On second rejection, stop and surface — something stranger is going on.
- **Never force-push.** Branch history integrity is load-bearing for
  review even though the branch commits don't reach `master`. Force-push
  semantics are a `/settle` rewrite concern, not `/yeet`.
- After push: `git status --short --untracked-files=all`. Any visible path
  means `/yeet` isn't done — continue classifying.

### 6. Report

Structured output: commits (sha + type + subject), paths removed /
ignored / moved with reasons, push target + result, final worktree status.

## Refuse Conditions

Stop and surface to the user instead of committing:

- `.git/MERGE_HEAD`, `.git/CHERRY_PICK_HEAD`, or any `rebase-*` dir exists
  — mid-operation.
- Diff contains unresolved conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`).
- Any file classified `secret-risk` (see § 2).
- HEAD is detached.
- Current branch is `master` AND the user did not explicitly ask to commit
  directly to master. Default: refuse, ask for a feature branch.
- Worktree has >500 files changed and no obvious semantic grouping — ask
  whether this is really one session's work.
- **Dagger pin drift**: `dagger.json` engineVersion changed in a commit that
  is not itself scoped `build:` (someone bumped the pin by accident).
  Refuse — surface for intentional bump in a dedicated `build:` commit.
- **OpenAPI ↔ router desync**: diff under `priv/openapi/` without a
  corresponding `lib/canary_web/router.ex` change (or vice versa) when the
  surface looks new. Flag as contract desync; ask the user before shipping.
- **`.codex/agents/*.toml` without role validation**: changes here require
  `dagger call codex-agent-roles` to pass. `./bin/validate --strict`
  enforces this at push — but surface the risk *before* running strict so
  the user knows what's coming.
- **Hot-path code without tests**: commit touches
  `lib/canary/errors/ingest.ex`, `lib/canary/webhooks/delivery.ex`,
  `lib/canary/health/manager.ex`, `lib/canary/query.ex`, or any file under
  `lib/canary/query/` WITHOUT a corresponding narrow test update. Surface.
- **Live DB file staged**: `canary.db`, `canary_dev.db`, `canary_test.db`,
  or `*.db-wal` / `*.db-shm` appears in `git status` as staged. Refuse —
  these are gitignored; presence means something is misconfigured.

## Safety rails (never)

- Never force-push. Branch-commit integrity matters for PR review.
- Never `--no-verify` to bypass `.githooks/pre-commit` or `.githooks/pre-push`.
  The gates ARE the product contract.
- Never `--amend` a commit that was rejected by a hook — create a new
  commit with the fix.
- Never `git add -A` at the repo root without classifying first. Canary has
  live DBs, thinktank secret-material dirs, and generated Dagger SDK cache
  that must not travel.
- Never `git clean -fdx` or delete directories without individual-file
  classification — scratch and evidence get moved out, not deleted.
- Never push to `master` from `/yeet` unless the user explicitly asked.
- Never open a PR, land a branch, or deploy — `/yeet` ends at push.
  Branch-landing is `/settle`; deploy is `/deploy`.
- Never commit files whose content matches the secret-risk patterns in § 2.
- Never declare success while `git status --short` still shows paths.

## Gotchas

- **"Tidy" is not refactor.** This skill stages and commits — it does not
  edit Elixir source to make it prettier. If the diff is messy, that's a
  `/refactor` concern, not `/yeet`.
- **Match the scope to the log, not a template.** Before picking a scope,
  grep `git log -30 --oneline` for the touched subtree's convention. If a
  new subtree appears with no precedent, use the most specific directory
  name that isn't overly long.
- **Untracked dirs are commonly overlooked.** `git add` doesn't recurse
  into new dirs by default unless you pass the dir path. Classify new
  dirs directory-by-directory — especially anything under `priv/`, `docs/`,
  or `backlog.d/`.
- **Evidence has a canonical home.** Canary keeps `design-catalog.html` at
  repo root as the QA-artifact pattern. New QA/demo artifacts either go
  under an established path or move to `~/vault/canary/evidence/`. Do not
  leave raw screenshots, logs, or walkthrough dumps in the worktree.
- **Pre-commit rewrites via `mix format`.** If `./bin/validate --fast`
  applies `mix format` inside a pre-commit flow, the formatted file is
  part of the commit. Don't panic and re-stage — accept the reformat.
- **Strict is slower than fast.** `./bin/validate --strict` (pre-push) runs
  dialyzer + sobelow + live advisories + coverage gates. Plan for it. Do
  not bypass to "save time."
- **Backlog archival is a `git mv`, not a copy.** When a commit closes
  `#NNN`, `git mv backlog.d/NNN-slug.md backlog.d/_done/NNN-slug.md` in
  the SAME commit and cite `#NNN` in the body.
- **The `gentle-working-tundra/` / `polished-marching-river/` /
  `sunlit-moving-walnut/` trap.** These are thinktank output dirs,
  gitignored. If `git status` shows them tracked, something broke — treat
  as secret-risk, refuse, and flag.
- **Governance edits are append-only.** `CLAUDE.md` footguns and `AGENTS.md`
  invariants grow; they don't get rewritten casually. Governance edits
  almost always want their own `chore(governance):` commit — not rolled
  into a feature.
- **Push rejection on first try is usually benign**: upstream moved.
  `git pull --rebase` + push once. Second rejection → stop.

## Output

```markdown
## /yeet Report

Classified 18 paths: 12 signal, 3 debris, 2 drift, 1 evidence.
Deleted: .DS_Store, _build/, thinktank.log
Moved out of repo: notes/query-split-ideas.md → ~/vault/canary/scratch/
Ignored: design-catalog.html (regenerated QA artifact, already in .gitignore)

Commits:
  abc1234 refactor(query): extract incident read model into Canary.Query.Incidents
  def5678 feat(query): add cursor pagination to incident read model (#010)
  9012345 chore(governance): document squash-merge policy in AGENTS.md

Per-commit strict: green (git rebase -x './bin/validate --strict').
Pushed feat/010-ramp-pattern-query-split → origin (3 new commits).
Worktree: clean
```

On refuse:

```markdown
## /yeet — REFUSED

Reason: canary_dev.db appears staged. Live SQLite files are gitignored
  for a reason — committing one leaks ingest/health state.
Action: `git restore --staged canary_dev.db`; rerun /yeet.
```

```markdown
## /yeet — REFUSED

Reason: priv/openapi/canary.yaml changed but lib/canary_web/router.ex
  has no matching pipeline edit. Contract ↔ router desync likely —
  either the route is missing or the OpenAPI bump is premature.
Action: resolve the desync (add the route, or back out the OpenAPI
  change), then rerun /yeet.
```
