# Canary Backlog

`backlog.d/` is the source of truth for active backlog work as of 2026-04-21.

## Priority Order

| # | Item | Priority | Status | Estimate |
|---|------|----------|--------|----------|
| 001 | Annotations API | high | done | M |
| 002 | Timeline agent polling | high | done | S |
| 003 | Triage diagnostic webhooks non-fatal | high | done | S |
| 004 | Incident correlation failure paths | high | done | S |
| 012 | Webhook delivery ledger + idempotency | high | done | M |
| 013 | Self-observability metrics export | high | done | M |
| 011 | OpenAPI spec + agent integration guide | high | done | M |
| 006 | Split Query into read models | high | done | L |
| 005 | Connect-a-service workflow | medium | done | M |
| 014 | Backup/restore + DR validation | medium | done | S |
| 008 | Security + governance baseline | medium | done | S |
| 007 | Networked service dogfooding | medium | done | L |
| 016 | Immutable CI control plane | medium | done | M |
| 017 | Dagger source contract + cache portability | medium | done | M |
| 018 | Local Docker probe hardening | medium | done | M |
| 019 | Dagger strict contract hardening | medium | done | S |
| 015 | Product security controls (scoped keys) | low | done | M |
| 009 | Desktop health semantics research | low | done | M |
| 021 | Check-in monitors for non-HTTP runtimes | medium | done | M |
| 022 | Contract hygiene + shallow-module collapse | high | done | M |
| 023 | Incident as atomic agent unit (detail API) | high | ready | M |
| 024 | Signal-agnostic annotations | medium | blocked | M |
| 025 | Audit test helpers for Ecto PK cast-drop | low | ready | S |
| 020 | Adminifi HTTP surface verification | low | blocked | S |
| 010 | Ramp pattern (north star) | high | blocked | XL |

## Dependency Map

```text
001 (annotations) ──┐
                    ├──→ 010 (ramp pattern) ──→ north star
002 (timeline)   ──┘        ↑
                    bb/011 (triage sprite) ──┘
                            ↑
012 (delivery ledger) ──────┘  load-bearing for agent consumers
003 (non-fatal webhooks) — prerequisite for sprite reliability
004 (correlation paths) — prerequisite for sprite signal quality
006 (query split) — enables cleaner annotation-aware queries
007 (dogfooding) — validates 001+002 on real workloads and unblocks 009
009 (desktop health semantics) — selects the non-HTTP model and unblocks 021
011 (OpenAPI) — contract for SDK convergence and agent self-discovery
013 (metrics) — self-observability for dogfooding credibility
014 (DR) — data durability assurance

022 (contract hygiene) ──── ships independently; restores summary invariant + supervision-tree collapse
023 (incident detail API) ──→ Canary-side substrate for bb/011 (and thus 010 ramp pattern)
024 (signal-agnostic annotations) ──→ blocked on 023; completes the Ramp-loop writable-metadata primitive
```

## Execution Lanes

**Lane 1 (agent readiness):** 012 (delivery ledger) → bb/011 (triage sprite) → 010 (ramp)
  · **023 (incident detail API) → 024 (signal-agnostic annotations)** land the Canary-side substrate bb/011 consumes
**Lane 2 (contract + observability):** 011 (OpenAPI) + 013 (metrics) — parallel, no deps
**Lane 3 (structural):** 006 (query split) → 005 (connect-a-service) · **022 (contract hygiene + shallow-module collapse)** — ship first of the active set; unblocks nothing but restores the summary invariant and sheds ~300 LOC of drift
**Lane 4 (hardening):** 008, 014, 016, 017, 018, 019 (independent, small, can ship anytime)
**Lane 5 (future):** 020 (Adminifi HTTP surface verification)

### Active order (2026-04-21)

1. **023** — incident detail endpoint (highest agent-first product impact)
2. **024** — signal-agnostic annotations (completes the ramp substrate, blocked on 023)

022 landed on 2026-04-21. 010 stays blocked on bb/011. 020 stays blocked on Adminifi URLs.

## Migration Notes

- Consolidated from `.backlog.d/` on 2026-03-30. Legacy items archived to `.backlog.d/_done/`.
- `.backlog.d/006` (monorepo bootstrap) archived as shipped — commit `c87f28f`.
- `.backlog.d/008` (monitor generation spike) superseded by 010-ramp-pattern.
- Bitterblossom triage sprite tracked at `bitterblossom/backlog.d/011-canary-triage-sprite.md`.
- 2026-04-02: Added 012–015 from multi-AI architecture audit. Promoted 006, 011 to high.
- 2026-04-21: Added 022–024 from grooming investigation (three parallel investigators: archaeologist / strategist / scout). Three themes: contract hygiene, incident-as-atomic-agent-unit, signal-agnostic annotations. 022 ready to ship first; 023 + 024 land the Canary-side substrate for the ramp pattern.

## Status

- `ready`: scoped and buildable now
- `blocked`: keep visible, but do not start before the listed dependency lands
- `done`: completed and archived under `_done/`
