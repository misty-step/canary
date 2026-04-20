---
name: harness
description: |
  Build, maintain, evaluate, and optimize the agent harness — skills, agents,
  hooks, CLAUDE.md, AGENTS.md, and enforcement infrastructure.
  Use when: "create a skill", "update skill", "improve the harness",
  "sync skills", "eval skill", "lint skill", "tune the harness",
  "add skill", "remove skill", "convert agent to skill",
  "audit skills", "skill health", "unused skills",
  "lint this skill", "eval this skill", "evaluate this skill",
  "validate this skill", "check this skill".
  Trigger: /harness, /focus, /skill, /primitive.
argument-hint: "[create|eval|lint|convert|sync|engineer|audit] [target]"
---

# /harness

Build and maintain the infrastructure that makes agents effective at shipping
Canary — the observability substrate for agent-driven infrastructure.

## Routing

| Intent | Reference |
|--------|-----------|
| Create a new skill or agent | `references/mode-create.md` |
| Evaluate a skill (baseline comparison) | `references/mode-eval.md` |
| Lint/validate a skill against quality gates | `references/mode-lint.md` |
| Convert agent ↔ skill | `references/mode-convert.md` |
| Sync primitives from spellbook to project | `references/mode-sync.md` |
| Design harness improvements | `references/mode-engineer.md` |
| Audit skill health and usage | `references/mode-audit.md` |

If first argument matches a mode name, read the corresponding reference.
If no argument, ask: "What do you want to do? (create, eval, lint, convert,
sync, engineer, audit)"

## Canary Harness Topology

**Two harnesses today, one source of truth.** Canary runs on Claude Code
(`.claude/`) and Codex (`.codex/`). Pi is not installed here. Any harness
mechanism must target both — filesystem + SKILL.md is the primary layer;
harness-native runtime features (Claude `enabledPlugins`, Codex `/plugins`)
are optimizations on top.

- **`.claude/skills/`** — installed skills. Mix of tailored workflow skills
  (ci, deploy, deliver, settle, implement, diagnose, groom, flywheel, qa,
  harness, refactor, code-review, monitor, shape, ceo-review, office-hours,
  reflect, research, model-research, agent-readiness, deps), spellbook
  universals (unmodified), and pre-existing project-specific skills
  (`canary`, `database`, `observability`, `security-scan`, `cli-reference`,
  `external-integration-patterns`, `github-cli-hygiene`, `git-mastery`,
  `design`, `design-review`, `high-end-visual-design`,
  `redesign-existing-projects`).
- **`.claude/agents/`** — installed personas. Core bench: `beck`, `builder`,
  `carmack`, `critic`, `grug`, `ousterhout`, `planner`. Specialist bench:
  `aesthetician`, `api-design-specialist`, `data-integrity-guardian`,
  `design-systems-architect`, `error-handling-specialist`, `fowler`,
  `infrastructure-guardian`, `observability-advocate`, `security-sentinel`,
  `test-strategy-architect`.
- **`.claude/settings.local.json`** — permissions allowlist. Currently
  minimal (five specific `Bash(...)` entries). Add specifically, not broadly.
  `Bash(*)` is a Red Line.
- **`.codex/agents/*.toml`** — Codex agent roles (one TOML per specialist
  persona, mirroring `.claude/agents/*.md`). Validated by Dagger:
  `dagger call codex-agent-roles` runs inside `./bin/validate --strict` and
  the hosted control plane. Adding or removing a specialist persona means
  updating **both** `.claude/agents/<name>.md` **and** `.codex/agents/<name>.toml`
  in the same change.
- **`.githooks/pre-commit`** → `./bin/validate --fast` (Dagger `fast` lane:
  lint + core tests).
- **`.githooks/pre-push`** → `./bin/validate --strict` (Dagger `strict` lane:
  full gate + advisories + Codex role validation + secrets scan).
- **`/Users/phaedrus/Development/spellbook/`** — source of truth for
  skills and agents. Edit upstream, then re-run `/tailor` (or `/seed` for a
  clean reinstall) to propagate. Never patch `.claude/` in place for anything
  that originated upstream.

## Spellbook Is The Source Of Truth

**Every tailored workflow skill and every agent persona is authored in
`/Users/phaedrus/Development/spellbook/` and installed into `.claude/` by
`/tailor` or `/seed`.** Fixing a bug in the canary copy is a Red Line —
the next `/tailor` run silently reverts it.

- *Create a workflow skill:* author under
  `spellbook/skills/<name>/SKILL.md`, then `/tailor` pulls a canary-tuned
  body into `.claude/skills/<name>/`. Never scaffold a tailored skill
  directly in `.claude/skills/`.
- *Update a workflow skill:* edit in spellbook, re-run `/tailor`. The
  tailored body should name `./bin/validate`, coverage thresholds
  (**81%** core, **90%** `canary_sdk`), responder boundary, and other
  canary invariants.
- *Add/remove an agent persona:* edit `spellbook/agents/<name>.md`, then
  re-sync. For specialist personas that exist in `.codex/agents/`, the
  matching `.toml` must be added or removed in lockstep and must still
  pass `dagger call codex-agent-roles`.
- *Pre-existing project-specific skills* (`canary`, `database`,
  `observability`, `security-scan`, `cli-reference`,
  `external-integration-patterns`, `github-cli-hygiene`, `git-mastery`,
  `design`, `design-review`, `high-end-visual-design`,
  `redesign-existing-projects`) are owned in-repo — they do not round-trip
  through spellbook. Edit them in `.claude/skills/<name>/` directly.

If `.claude/skills/<x>/SKILL.md` is byte-identical to
`spellbook/skills/<x>/SKILL.md` for a tailored workflow, the rewriter didn't
run — re-invoke `/tailor` for that skill.

## CLAUDE.md Is Append/Merge Only

**Hard rule.** `CLAUDE.md` has load-bearing **Footguns** and **Invariants**
sections (Ecto custom PKs, Oban Lite table creation, Req/Finch option
conflicts, ReadRepo exclusion from `ecto_repos`, Fly prod port binding,
Health.Manager boot resilience, SQLite WAL + `rm -f`). These cost us real
bugs to learn. Any harness-tuning mutation that touches `CLAUDE.md`:

- **Preserve Footguns and Invariants verbatim.** Never rewrite from scratch.
- **Append new footguns as they're discovered.** Don't reorganize — readers
  learn the order.
- **Merge, don't replace.** New sections go after existing ones unless the
  edit is a narrow in-place correction to something demonstrably wrong.
- **The Responder Boundary section is also load-bearing** — Canary owns
  ingest/health/correlation/timelines/queries/webhooks only. Repo mutation,
  issue creation, and LLM triage live in downstream responders. Do not
  let a harness edit blur that line.
- **The Deploy section's nuclear-reset recipe is load-bearing** — the
  stop → rm → restart ordering exists because of SQLite WAL. Preserve it.

If a skill asks you to "rewrite `CLAUDE.md`" — push back. The answer is
always an edit, never a rewrite.

## Cross-Harness Parity

Claude Code and Codex both scan SKILL.md from a skills directory. That's
the common denominator and the primary layer.

- New specialist persona: author spellbook agent, sync to
  `.claude/agents/<name>.md`, add matching `.codex/agents/<name>.toml`, run
  `./bin/validate --strict` locally to confirm `dagger call
  codex-agent-roles` passes before commit (pre-push will block otherwise).
- New workflow skill: spellbook author → `/tailor` → lands in
  `.claude/skills/<name>/`. Codex discovers it via the same filesystem —
  no per-harness wiring needed at the primary layer.
- If a proposed mechanism only works inside one harness's runtime (Claude
  plugin manifest, Codex `/plugins` command), find the filesystem-level
  equivalent first. Runtime-only designs are a bug.

## Audit & Lint Heuristics

When auditing installed skills, flag any of these as drift:

- Tailored workflow skill's body does not cite `./bin/validate` (with flag).
- Coverage threshold cited as anything other than **81%** (core) or
  **90%** (`canary_sdk`).
- Skill references "run CI" or "run the tests" as a bare phrase — canary's
  gate vocabulary is `./bin/validate` / `dagger call fast|strict`.
- Skill describes LLM-on-request-path flows (invariant violation) or
  dashboard-centric workflows (agents are the primary UI).
- Skill instructs repo mutation, issue creation, or LLM triage from inside
  Canary (responder boundary violation).
- Skill sources scripts from `$REPO_ROOT/…` or escapes its tree via
  `../..` (breaks symlink distribution — see `skills/<name>/lib/…` rule).
- `.claude/skills/` contains two SKILL.md files with the same frontmatter
  `name:`. Rename upstream in spellbook and re-`/tailor`; never rename in
  `.claude/`.
- Specialist persona present in `.claude/agents/` with no matching
  `.codex/agents/*.toml` (or vice versa).
- `.claude/settings.local.json` contains `Bash(*)` or other broad wildcards.
  Narrow to specific commands observed in the session.

## Skill Design Principles

These govern every mode. Quality standard for skills this harness creates,
evaluates, and lints.

1. **One skill = one domain, 1-3 workflows.** Three is healthy; five is a
   refactor signal.
2. **Token budget: 3,000 target, 5,000 ceiling.** Hard ceiling, not target.
3. **Mode content in references, not inline.** Mandatory for >3 modes. Thin
   SKILL.md with routing table; mode bodies in `references/mode-*.md`.
4. **Every line justifies its token cost.** Cut related-but-off-topic content
   first — it degrades more than unrelated noise.
5. **Description tax is always-on.** ~100 tokens per skill loaded every
   conversation. Don't split unless domain coherence demands it.
6. **Encode judgment, not procedures.** If the model already knows how, the
   skill is waste. Gotcha lists outperform happy-path pages.
7. **Mode-bloat gate.** >4 modes with inline content is a lint failure.
8. **Self-contained.** Every file the skill needs lives under
   `skills/<name>/`. Resolve via `$SCRIPT_DIR/lib/…`, never `$REPO_ROOT/…`.
   State roots resolve from `git rev-parse --show-toplevel` of the
   *invoking* project, not the skill's install location.
9. **Cross-harness first.** Target Claude Code + Codex (Pi-capable in the
   primary layer by construction). Runtime features are optimizations.
10. **Prose for an intelligent reader.** Favor bullets, examples, guardrails
    over flowcharts. Phase 0/Phase 1/state-machine shape means you're
    describing a program — strip to invariants + "shape of the work."

## Gotchas

- Skills that describe procedures the model already knows are waste.
- Descriptions missing trigger phrases won't fire.
- SKILL.md over 500 lines means progressive disclosure failed.
- Hooks that reference deleted skills silently break.
- Stale AGENTS.md instructions cause more harm than missing ones.
- After any model upgrade, re-eval skills — some become dead weight.
- Regexes over agent prose are proof the boundary is wrong.
- Editing `.claude/skills/<x>/SKILL.md` for a tailored workflow instead of
  spellbook → silently reverted on next `/tailor`.
- Adding a `.claude/agents/<name>.md` specialist without the matching
  `.codex/agents/<name>.toml` → `./bin/validate --strict` blocks pre-push.
- Rewriting `CLAUDE.md` instead of appending → load-bearing footguns lost,
  old bugs reappear.
- Anchoring a mechanism on Claude-only or Codex-only runtime features →
  parity bug. Filesystem + SKILL.md is the primary layer.
- Broadening `.claude/settings.local.json` with `Bash(*)` "to save prompts"
  → Red Line.
