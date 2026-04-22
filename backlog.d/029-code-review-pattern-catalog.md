# Code-review pattern catalog + reviewer wiring

Priority: medium
Status: ready
Estimate: S

## Goal

Stand up a living catalog of canary-specific code-review patterns at
`.agents/skills/code-review/references/canary-patterns.md` populated
from the last ~10 CodeRabbit/Gemini finds, wire `/code-review` to
consult it on every PR, and fold the three structural antipatterns
flagged by `/reflect prevent-coderabbit-patterns` into the CLAUDE.md
footgun list with one-line fixes. The catalog closes the gap for
semantic issues (missing nouns in summary templates, bounded-count vs
total-count, for-comprehension `{:ok, _}`-unwrap filter) where a
Credo check is the wrong tool.

## Non-Goals

- Replace Credo checks for statically-detectable issues
  (`#026`, `#027`).
- Replace the OpenAPI parity lane (`#028`).
- Import every historical CodeRabbit finding. Seed with the recurring
  patterns; grow on each cycle via `/reflect cycle`.
- Add a new reviewer agent or model. The existing critic panel inside
  `/code-review` already runs — this ticket only gives it a checklist.

## Oracle

- [ ] `.agents/skills/code-review/references/canary-patterns.md` exists
      with entries for at least: (a) summary-template invariants —
      "every bounded count has a noun," "pluralize never called with
      identical args," "totals in summaries derive from un-capped
      aggregates"; (b) test-scaffolding pitfalls — "for-comprehension
      with `{:ok, _}`-pattern filter silently swallows errors"; (c)
      contract-hygiene reminders — "OpenAPI schemas for nanoid PKs
      must be `string`, not `integer`"; (d) in-flight-bug pointers —
      "responder boundary: no LLM on request path, no repo mutation"
- [ ] Each entry has a title, one-sentence rule, a violating example
      pulled from real prior code, the fix, and a back-reference to
      the PR comment or CLAUDE.md section that first flagged it
- [ ] `.agents/skills/code-review/SKILL.md` is updated to require the
      reviewer subagent to load `canary-patterns.md` into its context
      before running a review, and to check each diff against the
      catalog
- [ ] `CLAUDE.md` footgun list gains three new entries:
      (i) preload-then-take on bounded read models,
      (ii) `:ets.foldl` + `:ets.delete` races (prefer
      `:ets.select_delete/2`),
      (iii) OpenAPI type for nanoid PK must be `string`, not
      `integer` — each with the enforcement mechanism noted
- [ ] A `/code-review` dry-run on the PR #133 diff (via
      `git diff master...8db76cc`) produces at least one finding that
      references an entry in the catalog, demonstrating the wiring
      works
- [ ] `./bin/validate --strict` green (pure docs / skill edits; no
      code changes)

## Notes

**Why now.** Of the ~15 CodeRabbit findings across PRs #132 and #133,
roughly a third are *semantic*, not syntactic: template drift, bounded
vs total, doc staleness, test-scaffolding weakness. A Credo check
can't catch these; a property test would be high cost for low
frequency. The right mechanism is the one CodeRabbit itself uses — a
checklist a reviewer consults on every diff. The `/code-review` skill
already dispatches critic subagents; they just aren't reading a
canary-specific playbook.

**Catalog format (per entry).**

```markdown
### P-NN — <one-line title>

**Rule.** <one sentence, imperative>

**Violating example** (from <PR or file:line>):
\`\`\`elixir
# bad
\`\`\`

**Fix.**
\`\`\`elixir
# good
\`\`\`

**Why it matters.** <2-3 sentences on consequence + detection notes>

**Enforcement.** <Credo check NNN / review checklist only / lane NNN>
```

Numbering starts at `P-01`; the ID is stable for cross-references from
PR comments and memory notes.

**Seed entries (first pass).**

- `P-01` — Summary template: every bounded count has a noun. (PR #133)
- `P-02` — Summary template: `pluralize/3` is never called with
  identical singular + plural. Test that asserts this directly if the
  pattern is worth hardening. (PR #133)
- `P-03` — Summary template: totals in summaries derive from
  un-capped aggregate queries, not truncated lists. (PR #132, PR #133)
- `P-04` — Test contract assertions: iterating
  `{:ok, result} <- responses` silently skips `{:error, _}`. Unwrap
  explicitly per entry. (PR #132)
- `P-05` — OpenAPI: nanoid primary keys are `string`, not
  `integer`. Enforced by `#028` when it lands; until then review
  checks. (PR #133)
- `P-06` — ETS TTL sweeps: prefer `:ets.select_delete/2` over
  `:ets.foldl` + `:ets.delete`. Enforced by a potential future Credo
  check; review caught this one. (PR #132)
- `P-07` — Preload-then-take on bounded read models. Enforced by
  `#027` when it lands. (PR #133)
- `P-08` — Docs: absolute paths (`/Users/…`, `/home/…`) never in
  committed markdown. Fix to repo-relative. (PR #132)
- `P-09` — Doc drift on mass changes: after deleting a
  surface/mode/feature, grep for stale references — common residuals
  live in sibling skills, README indexes, and header dates. (PR #132,
  PR #133)

**Reviewer wiring.**

In `.agents/skills/code-review/SKILL.md`, add a `Context loading`
section early in the workflow: the skill and all critic subagents
load `references/canary-patterns.md` before reviewing. For each
catalog entry, the reviewer checks whether the diff touches the
pattern's domain; if yes, emit a pass/fail note citing the entry ID.

Spellbook's `/code-review` skill will get a generalized "maintain a
local `review-patterns.md`" convention in `#048`; this ticket is the
canary instance.

**Execution sketch (one PR, three commits).**

*Commit 1 — `docs(code-review): seed canary-patterns.md with P-01…P-09`.*
New file at `.agents/skills/code-review/references/canary-patterns.md`.
Each entry hand-authored from the real PR comments; no lorem-ipsum.

*Commit 2 — `refactor(code-review): wire reviewer subagent to the catalog`.*
Edit `.agents/skills/code-review/SKILL.md`: add a `Context loading`
step; update the critic-dispatch prompt template to include
"Load references/canary-patterns.md before reviewing; cite entry IDs
on each finding."

*Commit 3 — `docs(ops): add three structural footguns to CLAUDE.md`.*
Preload-then-take, `:ets.foldl` + `:ets.delete`, OpenAPI-PK-integer.
Each with the one-line enforcement pointer (`#027`, manual, `#028`).

**Risk list.**

- *Catalog rots as patterns get merged into Credo checks.* Fine — when
  `#026`/`#027`/`#028` land, the relevant entries get an
  `Enforcement: Credo check <module>` line and remain in the catalog
  as documentation. Entries aren't deleted; enforcement advances.
- *Reviewer subagent ignores the catalog in practice.* Mitigated by a
  `/code-review` post-run assertion: every review synthesis must
  cite at least N P-IDs from the catalog (or explicitly note "no
  applicable patterns for this diff"). Add that assertion in Commit 2.
- *Canary-patterns.md drifts from reality.* `/reflect cycle` already
  runs after every `/flywheel` cycle; add a step where it checks
  whether any new CodeRabbit finds map to existing entries and
  appends new ones when none match.

**Lane.** Lane 4 (hardening). Pure-docs ticket; zero risk to
runtime code.

Source: `/reflect prevent-coderabbit-patterns` 2026-04-21, aggregated
from CodeRabbit + Gemini comments on PRs #132 and #133.
