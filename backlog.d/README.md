# Canary Backlog

`backlog.d/` is the source of truth for active backlog work as of 2026-06-11.

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
| 023 | Incident as atomic agent unit (detail API) | high | done | M |
| 024 | Signal-agnostic annotations | medium | done | M |
| 030 | Agent contract safety pass | high | done | M |
| 031 | Agent replay determinism hardening | high | done | M |
| 032 | Live Rust write-path evidence | high | done | L |
| 034 | Worker lifecycle readiness oracle | high | done | L |
| 033 | Deployed service registry lifecycle | high | done | M |
| 035 | Deployed app Canary coverage | high | done | XL |
| 036 | Agent-native inspection surface | high | done | L |
| 037 | Watch the watchmen | high | done | L |
| 038 | One-command integration agent | high | done | XL |
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
030 (agent contract safety) — depends on 011 + 012; makes scopes, summaries, cold-start guidance, annotation write-back, and webhook delivery replay machine-verifiable
031 (agent replay determinism) — shipped; malformed cursors, unsafe target cadence, invalid persisted probe methods, and unverifiable boot state fail explicitly before agents trust replay state
032 (live Rust write-path evidence) — follows the Rust production cutover; proves admin/ingest/webhook/monitor/target write paths with replayable evidence packets
033 (deployed service registry lifecycle) — shipped; owned-service monitoring state is timestamped and actionable, with blocked Adminifi and missing Vercel/Fly coverage captured in the registry
034 (worker lifecycle readiness oracle) — shipped; makes webhook, target, monitor, retention, and TLS workers visible to readiness/gate checks
035 (deployed app Canary coverage) — ensures every active owned Vercel/Fly deployment is enrolled or explicitly blocked with evidence
036 (agent-native inspection surface) — gives Codex/Claude a stable CLI/JSON/MCP-shaped way to inspect Canary status, errors, incidents, timelines, targets, and dogfood coverage
037 (watch the watchmen) — shipped; proves Canary itself from outside the Canary process, preserves receipts when Canary is unreachable, and surfaces the external witness in `bin/canary doctor`
038 (one-command integration agent) — discovers, patches, enrolls, and verifies Canary integration for Vercel/Fly/Next apps

022 (contract hygiene) ──── ships independently; restores summary invariant + supervision-tree collapse
023 (incident detail API) ──→ Canary-side substrate for bb/011 (and thus 010 ramp pattern)
024 (signal-agnostic annotations) ──→ blocked on 023; completes the Ramp-loop writable-metadata primitive
```

## Execution Lanes

**Lane 1 (agent readiness):** 012 (delivery ledger) → bb/011 (triage sprite) → 010 (ramp)
  · **023 (incident detail API) → 024 (signal-agnostic annotations)** land the Canary-side substrate bb/011 consumes
**Lane 2 (contract + observability):** 011 (OpenAPI) + 013 (metrics) — parallel, no deps · **030 (agent contract safety)** depends on 011 + 012 and tightens the existing contract for autonomous consumers · **031 (agent replay determinism)** shipped the malformed replay/query/health contract errors · **032 (live Rust write-path evidence)** proves the Rust production surface beyond read-only smoke
**Lane 3 (structural):** 006 (query split) → 005 (connect-a-service) · **022 (contract hygiene + shallow-module collapse)** — ship first of the active set; unblocks nothing but restores the summary invariant and sheds ~300 LOC of drift
**Lane 4 (hardening):** 008, 014, 016, 017, 018, 019 (independent, small, can ship anytime) · **034 (worker lifecycle readiness oracle)** hardens the Rust background-worker proof surface
**Lane 5 (dogfood coverage):** 020 (Adminifi HTTP surface verification) · **033 (deployed service registry lifecycle)** shipped the managed registry substrate · **035 (deployed app Canary coverage)** makes every active owned deployment covered or explicitly blocked · **036 (agent-native inspection surface)** gives agents the operating view · **037 (watch the watchmen)** proves Canary externally · **038 (one-command integration agent)** removes setup friction

### Active order (2026-06-11)

No ready active items remain. 020 stays blocked on Adminifi URLs; 010 stays
blocked on the downstream bitterblossom triage sprite.

022 + 023 landed on 2026-04-21. 024 landed on 2026-04-22. 026 landed on
2026-04-23 — Ramp
substrate now complete; bb/011 unblocks the north star. Elixir-era lint and
parity backlog items were retired during the Rust scorched-earth migration.
010 stays blocked on bb/011. 020 stays blocked on Adminifi URLs.

## Migration Notes

- Consolidated from `.backlog.d/` on 2026-03-30. Legacy items archived to `.backlog.d/_done/`.
- `.backlog.d/006` (monorepo bootstrap) archived as shipped — commit `c87f28f`.
- `.backlog.d/008` (monitor generation spike) superseded by 010-ramp-pattern.
- Bitterblossom triage sprite tracked at `bitterblossom/backlog.d/011-canary-triage-sprite.md`.
- 2026-04-02: Added 012–015 from multi-AI architecture audit. Promoted 006, 011 to high.
- 2026-04-21: Added 022–024 from grooming investigation (three parallel investigators: archaeologist / strategist / scout). Three themes: contract hygiene, incident-as-atomic-agent-unit, signal-agnostic annotations. 022 ready to ship first; 023 + 024 land the Canary-side substrate for the ramp pattern.
- 2026-05-19: Groomed stale active backlog. Archived 025 as subsumed by #026
  and archived shipped 027. Added 030 from the agent-contract safety theme:
  per-operation scope metadata, summary completeness discipline, cold-start
  guidance, annotation write-back conventions, and delivery-id-addressable
  webhook diagnostics without crossing the responder boundary.
- 2026-05-24: Groomed toward usefulness/elegance: promoted #030, added #031
  for deterministic replay/health/readiness boundary failures, and clarified
  that #010 is now blocked on the downstream bitterblossom triage sprite rather
  than Canary-side annotation/timeline substrate.
- 2026-06-07: Retired Elixir-era active backlog items during the Rust
  scorched-earth migration; Rust-owned Dagger, OpenAPI, and cargo tests are now
  the active contract surfaces.
- 2026-06-11: Groomed the current Rust backlog from `origin/master`, archived
  031 after verifying the Rust replay/health determinism guardrails, kept 030
  focused on missing OpenAPI operation-level scope/guidance metadata, and added
  032-034 for live write-path proof, dogfood registry lifecycle, and worker
  readiness observability.
- 2026-06-11: Follow-up dogfood/watchmen design pass from live Vercel, Fly, and
  Canary evidence. Promoted 033 from static dogfood buckets to a deployed
  service registry, then added 035-038 for exhaustive owned-app coverage,
  agent-native inspection, independent Canary witnessing, and one-command
  integration.
- 2026-06-11: Delivered 033. `priv/dogfood/owned_services.json` is now a
  schema-versioned deployed-service registry; `bin/dogfood-audit` validates it,
  emits text or JSON, and strict-checks active services against live Canary.
- 2026-06-11: Delivered 036. `bin/canary` now wraps the Rust `canary-cli`
  inspection surface with text/JSON commands for summary, services, errors,
  incidents, timeline, targets, monitors, dogfood audit, doctor, and MCP tool
  manifest generation.
- 2026-06-12: Delivered 037. `bin/canary-witness` now checks `/healthz`,
  `/readyz`, and `service=canary` readback from GitHub Actions, preserves a
  receipt artifact outside Canary, sends `canary-watchman` check-ins when
  healthy, and surfaces witness state in `bin/canary doctor`.
- 2026-06-12: Delivered 038. `bin/canary integrate` now discovers local
  project coverage without reading secret values, emits reviewable patch and
  enrollment plans, safely patches Next.js apps, enrolls deployed health
  targets through service onboarding, and exposes the loop through the MCP
  manifest.
- 2026-06-12: Delivered 030. The checked-in OpenAPI contract now carries
  machine-readable least-privilege scope metadata on authenticated operations,
  cold-start and annotation write-back guidance for agents, a documented
  summary-exception table, and contract tests tying the spec to Rust route
  operations, summary coverage, primary agent guidance, and delivery lookup.
- 2026-06-12: Delivered 032. `bin/canary-write-path-rehearsal` now replays the
  live Rust admin/ingest/target/monitor/webhook/delivery-ledger/readback/DR
  write paths with redacted JSON receipts, and
  `docs/architecture/rust-write-path-evidence-2026-06-12.md` records the first
  production run plus cleanup proof.
- 2026-06-13: Delivered 034. `/readyz` now exposes process-local lifecycle
  snapshots for webhook delivery, target probes, monitor overdue evaluation,
  retention pruning, and TLS scanning; worker loops record visible sanitized
  failure counters, and Dagger production smoke asserts all five workers are
  present and started.

## Status

- `ready`: scoped and buildable now
- `blocked`: keep visible, but do not start before the listed dependency lands
- `done`: completed and archived under `_done/`
