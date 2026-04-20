---
name: groom
description: |
  Backlog management, brainstorming, architectural exploration for Canary.
  File-driven via backlog.d/ (priority + lanes in backlog.d/README.md).
  Parallel investigation bench, synthesis protocol, themed recommendations
  anchored to the responder boundary and the ramp-pattern north star.
  Use when: backlog session, "groom", "what should we build", "rethink this",
  "biggest opportunity", "backlog", "prioritize", "tidy", "scaffold".
  Trigger: /groom, /backlog, /rethink, /moonshot, /scaffold, /tidy.
argument-hint: "[explore|rethink|moonshot|scaffold|tidy] [context]"
---

# /groom (canary)

Strategic backlog management for Canary's observability substrate. Parallel
investigation, synthesis, themed recommendations — ranked against the
**agent-first** product vision (`VISION.md`) and the `backlog.d/README.md`
Lanes 1–5.

Scaffold mode is **not applicable here** — Canary is already scaffolded
(Elixir/OTP app + Dagger pipeline + `./bin/validate` gate + SQLite + Fly
deploy). Use `/harness` instead for harness-level bootstrap work.

## Execution stance

You are the executive orchestrator.
- Keep synthesis, prioritization, and recommendation decisions on the lead
  model. Product judgment is not delegable.
- Delegate investigation + evidence gathering to focused subagents.
- Run independent investigators in parallel by default (`Agent` tool, single
  message, three tool uses).

## Modes

| Mode | Intent |
|------|--------|
| **explore** (default) | Parallel investigation → synthesized themes → prioritized `backlog.d/NNN-*.md` items |
| **rethink** | Deep architectural exploration of a target system → one clear recommendation |
| **moonshot** | Strategist ignores current backlog and thinks from first principles against the VISION.md north star |
| **tidy** | Prune, reorder, archive completed items to `backlog.d/_done/`, re-verify priority/dep maps |

## Backlog format: `backlog.d/`

```
backlog.d/
├── README.md                            # Priority table + dep map + Lanes 1–5
├── 010-ramp-pattern.md                  # active (blocked, XL, north-star)
├── 020-adminifi-http-surface-verification.md
└── _done/                               # 21 archived items as of 2026-04-20
    ├── 001-annotations.md
    └── ...
```

**File naming.** `NNN-<slug>.md` — zero-padded to 3 digits. Next ID =
max(existing) + 1 (across active + `_done/`). Cross-repo dep IDs use a
prefix: `bb/011-canary-triage-sprite.md` (bitterblossom-side).

**Item shape** (see any `backlog.d/_done/*.md` for exemplars):

```markdown
# <Short title>

Priority: high | medium | low
Status: pending | ready | blocked | in-progress | done | shipped | abandoned
Estimate: S | M | L | XL

## Goal
<1 sentence — agent-observable outcome, not mechanism>

## Non-Goals
- <explicitly out-of-scope — especially anything that crosses the
  responder boundary: repo mutation, issue creation, LLM triage>

## Oracle
- [ ] <mechanically verifiable, prefer executable commands>
- [ ] <e.g. `curl -s https://canary-obs.fly.dev/api/v1/query?service=X` returns `summary` field>

## Notes
<context, constraints, CLAUDE.md footgun cite if relevant, related issues>
```

**Responder-boundary gate** (shape rule). Every new item is checked against
`CLAUDE.md` > "Responder Boundary":

- Canary owns: **ingest, health/check-in, correlation, timelines, queries,
  signed generic webhooks**.
- Consumers own: **repo mutation, issue creation, LLM triage**.

If a proposed item has Canary mutating a consumer's repo or firing an
opinionated (non-generic) webhook, **reject the framing** and move the
repo-mutation half to a downstream backlog (typically bitterblossom).
`#010 Ramp pattern` is explicitly blocked on this boundary — the triage
sprite lives downstream, not in canary.

**Closure markers for manual landings.** Ships outside `/flywheel` should
carry one of these in the landing commit so `tidy` can detect stale items:
- `Closes backlog:<item-id>`
- `Ships backlog:<item-id>`

## Context loading (all modes except tidy)

Before investigation, the orchestrator gathers baseline context. Takes
<2 minutes. Note absences, don't block.

1. **Read `backlog.d/README.md`** — the priority table, dep map, and
   Lanes 1–5 are load-bearing. The next investigation must respect them.
2. **Read active items.** `backlog.d/*.md` minus `_done/`. As of
   2026-04-20: `010-ramp-pattern.md` (blocked, XL, north-star) and
   `020-adminifi-http-surface-verification.md` (blocked, S).
3. **Read `VISION.md` + `PRINCIPLES.md`.** Canary's ranking function is
   "does this make it easier for an AI agent to understand system health,
   diagnose issues, and take action?" Nothing else.
4. **Read `CLAUDE.md`.** Footgun list is the source of truth for recurring
   failure modes. If an investigator surfaces a "new" bug pattern that's
   actually already in the footgun list, that's a sign the footgun wasn't
   surfaced in shape/review — flag as a harness issue, not a backlog item.
5. **Skim `.spellbook/repo-brief.md`.** Shared spine for what this repo is.
6. **Read recent git log** (`git log --oneline -30`). Anchor the investigators
   in the last two weeks of actual work.
7. **Cap check.** Canary keeps the backlog small (active items typically
   <5). If explore would push over **15 active items**, declare a reduction
   session (no new items until `tidy` or shipping brings it back under).
8. **Ask the user:** "Anything on your mind? Footgun you've hit twice?
   Deploy that's been flaky? Area where the responder boundary is blurring?"

## Investigation bench

Three named investigators per mode, all launched **in parallel** via the
Agent tool in a single message. One tool call = three `Agent` uses.

**MANDATORY PARALLEL FANOUT.** A grooming session that runs one investigator
has failed the investigation goal. All three, every time.

### Explore investigators

| Investigator | Lens | Mandate | Agent type |
|---|---|---|---|
| **Archaeologist** | Codebase health (Canary-specific) | Read `ARCHITECTURE.md` module map + `lib/canary/**`. Surface: complexity hotspots in `query/`, `health/`, `alerter/`, `errors/`, `workers/`; footgun recurrences from `CLAUDE.md`; coverage gaps against 81% core / 90% canary_sdk gates; supervision tree fragility. "What's fragile? What's violating the single-writer invariant?" | Explore |
| **Strategist** | Product opportunity (agent-first) | Read `VISION.md`, `PRINCIPLES.md`, `.spellbook/repo-brief.md`, `priv/openapi/*`. Surface: missing agent-facing capabilities, OpenAPI `info.x-agent-guide` gaps, `summary` field completeness, scoped-key coverage, generic-webhook replay parity. "What would an agent consumer pay more for?" | Explore |
| **Velocity** | Effort patterns (last 30 days) | Read `git log --oneline --since='30 days ago'`, `backlog.d/_done/`, uptime-monitor workflow runs, fly releases. Surface: fix-to-feature ratio, churn hotspots (paths modified >3× per week), stalled work (`in-progress` with no commits >7d). "Where is effort not producing agent-observable value?" | Explore |

For **moonshot** mode: the Strategist prompt becomes — "Forget
`backlog.d/*.md`. What's the single highest-leverage thing Canary is not
building, anchored to the VISION.md 'canonical interface between AI agents
and production infrastructure' north star?"

### Rethink investigators

| Investigator | Lens | Mandate | Agent type |
|---|---|---|---|
| **Mapper** | System topology | Trace coupling in the target system. For `lib/canary/query/*`: follow `Canary.Query` → ReadRepo → error_groups + targets + incidents. For `lib/canary/health/*`: Manager → Supervisor → Checker → StateMachine. For `lib/canary/alerter/*`: Signer → CircuitBreaker → Cooldown → WebhookDelivery Oban. "What breaks if you pull any thread?" | Explore |
| **Simplifier** | Radical simplicity | From-scratch perspective on the target. For Canary, the simplifier anchor is `PRINCIPLES.md` #7 "Code is a liability" and the v1 constraint of "one Docker image, one SQLite file, one config." "What layers can be deleted without violating an invariant?" | Plan |
| **Scout** | External perspectives | Invokes `/research thinktank` on the target. For health/probing: "what have Uptime Robot / Pingdom / Better Stack learned about probe scheduling and consensus?" For alerter: "how do Sentry / PagerDuty handle flapping and cooldown?" For ingest: "what are the patterns agent-first observability should borrow from OpenTelemetry?" | general-purpose |

### Investigator output format (shared)

Every investigator returns this exact shape:

```markdown
## [Name] Report
### Top 3 findings
1. <finding> — Evidence: <file:line / commit SHA / metric>. Impact: high | med | low.
2. ...
3. ...
### Strategic theme
<one sentence: the overarching theme these findings point to>
### Single recommendation
<one concrete action. Not a list. Not "consider." A specific thing to do
in Canary's codebase, cited to files.>
```

## Synthesis protocol

After all investigators return, the **orchestrator** (you) synthesizes. Do
NOT present raw findings. Do NOT delegate synthesis to a subagent — this
requires product judgment and the responder-boundary test.

0. **Premise challenge.** Audit the request's framing before theming. Is
   this the root problem or a downstream symptom? Five-whys the stated
   goal. Example: "add a GitHub integration for webhook events" → five
   whys → "consumers want one-click wire-up" → but Canary's doctrine
   (`PRINCIPLES.md` #3 Broadcast, Don't Prescribe) says consumers wire
   their own behavior. Re-anchor: the real ticket is "improve generic
   webhook replay + SDK samples," not "add an integration."
1. **Responder-boundary gate.** For each candidate theme, apply the gate
   from the Backlog format section. Anything that puts Canary on the
   mutate-downstream side is reframed or split; downstream half lives
   elsewhere (usually bitterblossom).
2. **Cross-reference.** Which findings appear across 2+ investigators?
   (Highest signal.)
3. **Theme extraction.** Group findings into 2–4 strategic themes. A theme
   is a cluster sharing a root cause or shared solution. Not individual
   items — themes.
4. **Dependency map.** Do any themes depend on others? Check against the
   active `backlog.d/README.md` dep map (e.g., `bb/011` blocks `010`;
   `#012 delivery ledger` was load-bearing for agent consumers).
5. **Rank.** Order by `(impact on product vision) × (feasibility) ÷ (effort)`.
   Vision-alignment gets 2× weight — Canary's ranking function is
   "easier for an AI agent to {understand, diagnose, act}." If a theme
   doesn't score there, drop it to the bottom or reject.
6. **Present.** One theme at a time. Evidence from investigators,
   recommended action, rough effort (S/M/L/XL). Ask the user: explore
   deeper, write backlog item, or skip?

Output format:

```markdown
## Grooming synthesis

### Investigator convergence
<findings from 2+ investigators — highest signal>

### Theme 1: <Name>
**Evidence:** Archaeologist found X, Strategist found Y, Velocity confirms Z.
**Responder boundary:** <"clean — canary-side only" | "split — Y half goes
 to bitterblossom">
**Recommendation:** <one concrete action, cited to files>
**Effort:** S | M | L | XL
**Agent impact:** <which of {understand, diagnose, act} does this improve>

### Theme 2: ...

### Dependency order
Theme A enables Theme B. Recommend executing A first.
```

## Workflow: explore

Phase-gated. Each phase must complete before the next begins.

### 1. CONTEXT — load baseline (see Context loading)
### 2. INVESTIGATE — launch all three explore investigators in parallel
Gate: all three returned structured reports.
### 3. SYNTHESIZE — cross-reference, theme, rank, boundary-gate
Gate: themes extracted with evidence, recommendations, and clean boundary.
### 4. DISCUSS — present one theme at a time. Recommend, don't list.
Gate: user decides per theme (explore deeper / write item / skip).
### 5. WRITE — create `backlog.d/NNN-<slug>.md` for approved themes
Each item: Goal + Non-Goals + Oracle + Notes. Every Oracle prefers
executable verification: a `curl` against `canary-obs.fly.dev`, a
`mix test` against a specific test file, a `dagger call fast/strict`
invocation, a webhook delivery that produces a `X-Delivery-Id`.

Update `backlog.d/README.md` in the same commit:
- Add the row to the priority table.
- Add the dep-map edge if any.
- Add to the appropriate Lane (1 agent readiness / 2 contract + obs /
  3 structural / 4 hardening / 5 future).

Gate: every item has Goal + Non-Goals + Oracle; `README.md` table
updated; conventional-commit scoped `chore(backlog):`.

### 6. PRIORITIZE — reorder `backlog.d/README.md` table by value/effort

## Workflow: rethink

### 1. CONTEXT — load baseline + user specifies the target system
(e.g. "rethink `lib/canary/health/*`", "rethink the alerter")
### 2. INVESTIGATE — launch all three rethink investigators in parallel
Gate: all three returned structured reports.
### 3. SYNTHESIZE — distill into 2–3 architectural options with honest tradeoffs
Always include "do nothing" as a viable option. Each option is checked
against the invariants list in `AGENTS.md`: pool_size:1, pure StateMachine,
RFC 9457 Problem Details, scoped API keys, responder boundary, no
hardcoded service names, Target vs Monitor distinction.
### 4. RECOMMEND — pick one option. Argue for it. Be opinionated.
Gate: one clear recommendation with reasoning anchored to the invariants.
### 5. DISCUSS — user approves, modifies, or rejects
### 6. WRITE — one `backlog.d/NNN-*.md` item for the recommended change

## Workflow: tidy

1. **Backlog audit** — `ls backlog.d/*.md | wc -l` (excluding `_done/` +
   `README.md`). Against the 15-item cap.
2. **Archive completed items** (`Status: done` or `Status: shipped`) to
   `backlog.d/_done/` via `git mv backlog.d/NNN-*.md backlog.d/_done/`.
3. **Archive active items** that already carry `## What Was Built` or are
   closed by current-branch commit markers (`Closes backlog:NNN`,
   `Ships backlog:NNN`). Scan via
   `git log --oneline master..HEAD | grep -E 'Closes backlog|Ships backlog'`.
4. **Delete stale items** — `>30 days untouched, no longer relevant`.
   Check last-touched via `git log -1 --format=%cd backlog.d/NNN-*.md`.
5. **Flag zombies** — items in `Status: in-progress` with no commits
   touching related paths in >7d. These are abandoned, not active.
6. **Verify invariants per item:** Goal present, Oracle present, Non-Goals
   present for anything medium-or-larger, responder boundary explicit if
   the item touches webhooks / ingest / consumer surfaces.
7. **Verify done items have `## What Was Built`** — if not, synthesize
   one from the landing commit and the diff.
8. **Reorder `backlog.d/README.md` priority table** — blocked items drop
   below active; north-star items stay at top of their lane.
9. **Dep-map audit** — walk the ASCII arrow graph; drop edges to items
   that moved to `_done/`; surface any new cross-repo deps
   (`bb/011`, etc.).
10. **Commit scoped `chore(backlog):`.** Example from git log:
    `26c45f3 chore(backlog): archive 16 shipped items to _done/`.

## Gotchas

- **Accepting the ticket framing as given.** A `/groom X` request is the
  user's first-draft articulation, not a locked problem statement. Five-
  whys before investigating.
- **Proposing items that violate the responder boundary.** "Canary opens a
  GitHub PR on alert" is not a Canary item — it's a bitterblossom item.
  `PRINCIPLES.md` #3 is non-negotiable.
- **Investigators returning "everything is fine."** Red flag. Push harder.
  Canary has footguns; an Archaeologist that found none didn't look.
- **Synthesis that lists findings without theming.** That's a report, not
  synthesis. Group into themes before presenting.
- **Themes without recommendations.** That's a menu, not grooming. Pick
  one action per theme and argue for it.
- **Running one investigator and calling it done.** Mandatory parallel
  fanout. All three, every time.
- **Items without oracles.** If you can't write an executable verification
  (`curl`, `mix test`, `dagger call`), the item isn't scoped.
- **Items that don't improve any of {understand, diagnose, act} for
  agents.** Score the theme against the ranking function. If zero, drop.
- **Backlog as graveyard.** Items >30 days old with no progress are dead.
  Archive or delete during tidy.
- **New bug pattern that's already in `CLAUDE.md` footguns.** That's a
  harness failure (shape/review didn't cite the footgun), not a backlog
  item. Route to `/reflect` + `/harness`, not `backlog.d/`.

## Principles

- **Investigate before opining** — parallel investigation first, opinions after evidence
- **Boundary-gate before theming** — responder boundary catches bad tickets early
- **Theme, don't itemize** — strategic themes, not feature laundry lists
- **Recommend, don't list** — always have an opinion, argue for it, anchored to invariants
- **One theme at a time** — don't overwhelm during discussion
- **Agent-first is the ranking function** — rank by `{understand, diagnose, act}` impact
- **Every item needs an executable oracle** — if a `curl` or `mix test` can't verify done, the item isn't ready
- **File-driven** — `backlog.d/` is the source of truth; `backlog.d/README.md` priority table is canonical
