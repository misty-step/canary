# Canary Backlog

`backlog.d/` is the source of truth for active backlog work as of 2026-03-30.

## Priority Order

| # | Item | Priority | Status | Estimate |
|---|------|----------|--------|----------|
| 001 | Annotations API | high | done | M |
| 002 | Timeline agent polling | high | ready | S |
| 003 | Triage diagnostic webhooks non-fatal | high | done | S |
| 004 | Incident correlation failure paths | high | in-progress | S |
| 005 | Connect-a-service workflow | high | ready | M |
| 006 | Split Query into read models | medium | ready | L |
| 007 | Networked service dogfooding | medium | ready | L |
| 008 | Security + governance baseline | medium | ready | S |
| 009 | Desktop health semantics research | low | blocked | M |
| 010 | Ramp pattern (north star) | high | blocked | XL |

## Dependency Map

```text
001 (annotations) ──┐
                    ├──→ 010 (ramp pattern) ──→ north star
002 (timeline)   ──┘        ↑
                    bb/011 (triage sprite) ──┘
003 (non-fatal webhooks) — prerequisite for sprite reliability
004 (correlation paths) — prerequisite for sprite signal quality
006 (query split) — enables cleaner annotation-aware queries
007 (dogfooding) — validates 001+002 on real workloads
```

## Execution Lanes

**Lane 1 (agent readiness):** 001 + 002 (parallel) → bb/011 → 010
**Lane 2 (hardening):** 003, 004 (independent, small, can ship anytime)
**Lane 3 (structural):** 005, 006, 007, 008 (ready but lower priority)

## Migration Notes

- Consolidated from `.backlog.d/` on 2026-03-30. Legacy items archived to `.backlog.d/_done/`.
- `.backlog.d/006` (monorepo bootstrap) archived as shipped — commit `c87f28f`.
- `.backlog.d/008` (monitor generation spike) superseded by 010-ramp-pattern.
- Bitterblossom triage sprite tracked at `bitterblossom/backlog.d/011-canary-triage-sprite.md`.

## Status

- `ready`: scoped and buildable now
- `blocked`: keep visible, but do not start before the listed dependency lands
- `done`: completed and archived under `_done/`
