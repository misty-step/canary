---
name: canary repo brief
description: Shared spine for tailored harness primitives — the non-duplicate narrative; invariants + gate + debts live in AGENTS.md
last-updated: 2026-04-20
---

# Canary — Repo Brief

Narrative spine for tailored primitives in `.agents/skills/`. **Stack,
gate contract, invariants, and known-debt map live in `AGENTS.md`** (the
router); this brief holds only the content that router tables can't carry
— vision prose, terminology, session signal, and user-ratified patterns.

Do not duplicate invariant lists or the gate table here. If a rewriter
needs them, they cite `AGENTS.md` directly.

## Vision in one breath

Canary is an **agent-first** observability substrate — error ingestion +
health + check-in monitoring — for AI agents, not human operators. Agents
consume `summary` fields, the OpenAPI `info.x-agent-guide`, signed generic
webhooks, and scoped API keys. The operator `/dashboard` LiveView exists
as a fallback (password-gated), not as the product surface. The ranking
function for every backlog item is: *does this make it easier for an AI
agent to **understand** system health, **diagnose** issues, or **act** on
them?* Full vision: `VISION.md`. Operating principles: `PRINCIPLES.md`.

## North star

The **ramp pattern** (`backlog.d/010-ramp-pattern.md`): error →
auto-triage → fix, closed by a downstream triage sprite
(`bitterblossom/backlog.d/011-canary-triage-sprite.md`). Canary owns the
substrate; the consumer owns the action. The responder boundary is
how we keep those halves separable.

## Terminology

- **Target** = HTTP probe subject (GenServer per target via
  `Canary.Health.Manager`). Not a "monitor" in Canary vocabulary.
- **Monitor** = non-HTTP check-in watcher (Oban-scheduled, product
  concept). Distinct from `/monitor` the harness skill.
- **Ingest** = `Canary.Errors.Ingest.ingest/1`. Validate → group_hash →
  persist → webhook.
- **Ramp pattern** = error → auto-triage → fix loop; Canary's north-star UX.
- **Triage sprite** = bitterblossom-side agent consumer that closes the
  ramp loop. Not in this repo.
- **Gate** = `./bin/validate` and only `./bin/validate`. "CI" in prose
  refers to the hosted GitHub workflow, which runs the same gate.
- **Dogfood audit** = `./bin/dogfood-audit [--strict]` — canary watching
  its own networked services.
- **Responder boundary** = the line between what Canary owns
  (ingest/health/correlation/timelines/queries/generic-webhooks) and what
  consumers own (repo mutation / issue creation / LLM triage). Enforced
  at shape time in every new backlog item.

## Session signal

**Recurring user corrections / validated patterns** (from session
history + memory):

1. **Every product is demoable.** Don't skip `/demo` on "no marketing
   surface" / "operator-only dashboard" grounds. API + SDK + webhook
   replay counts. (Memory: `feedback_demoable_scope`.)
2. **Distinguish skills by operational shape, not name.** `/yeet` (worktree
   → push) vs `/settle` (branch landing) are distinct, even though both
   "ship." Same for `/demo` vs `/qa`.
3. **Custom string PKs must be set on the struct, not cast.** Ecto
   silently drops `id` fields that aren't in `@required`/`@optional`.
   Six bugs in initial build. (Memory: `feedback_ecto_custom_pks`.
   Full footgun list: `CLAUDE.md`.)
4. **Oban Lite does not auto-create its tables.** Use a dedicated Ecto
   migration with `execute`, never a GenServer + `Repo.query!` (races
   with `pool_size: 1`). (Memory: `feedback_oban_sqlite`.)
5. **Project skills belong in the shared repo skill root**, not global
   `~/.claude/skills/`. (Memory: `feedback_skill_placement`.)
6. **Harness primitives are code.** `.agents/`, `.codex/`, `.claude/`,
   `.spellbook/repo-brief.md` are version-controlled — they are the
   contract between the codebase and the AI, not ephemera.

## Validated approaches (user-ratified, keep using)

- Router-style `AGENTS.md` with stack/boundaries, ground-truth pointers,
  invariants, gate contract, known-debt map, harness index tables. Not
  a prose codex.
- Single gate citation of `./bin/validate` (never "run CI" / "run the
  tests"); coverage names `81% core / 90% canary_sdk`;
  conventional-commit scopes taken from `git log` (they land on the
  PR-title / squash-subject, not necessarily every branch commit);
  squash-merge via `gh pr merge --squash`; responder boundary enforced
  in acceptance criteria.
- Load-bearing footgun list stays in `CLAUDE.md`. Never duplicate into
  `AGENTS.md` or skill bodies — cite it.
- Shared-root skill layout: `.agents/skills/<name>/` canonical;
  `.claude/skills/<name>` and `.codex/skills/<name>` as relative symlink
  bridges. Edit once, both harnesses pick up.
