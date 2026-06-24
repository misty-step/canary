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
| 041 | Live integration verification harness | P0 | done | L |
| 042 | Runtime pressure and freshness operations | P0 | done | L |
| 040 | Universal integration and enrollment engine | P0 | done | XL |
| 043 | Agentic remediation claim protocol | P1 | done | L |
| 044 | Telemetry and analytics signal model | P1 | done | XL |
| 045 | Self-watch operator verdict | P0 | done | L |
| 046 | Dogfood value receipts | P0 | done | L |
| 047 | Alert-plane reliability and SLO burn-rate | P0 | ready | XL |
| 048 | Responder rich-context safety gate | P0 | pending | XL |
| 049 | Integration evidence and capture gaps | P1 | pending | XL |
| 050 | Cold-agent readiness proof | P1 | pending | M |
| 051 | TypeScript SDK npm publish | P1 | pending | S |
| 052 | Runnable MCP server | P1 | pending | M |
| 053 | Human alert delivery (decision) | P2 | pending | M |
| 054 | Serving model: self-hosted, managed-later, not multi-tenant SaaS | P2 | pending | S |
| 055 | Refresh PRINCIPLES.md examples to Rust+SQLite (post-cutover) | P3 | pending | S |
| 020 | Adminifi HTTP surface verification | low | blocked | S |
| 010 | Ramp pattern (north star) | high | blocked | XL |

## Dependency Map

```text
001 (annotations) ──┐
                    ├──→ 010 (ramp pattern) ──→ north star
002 (timeline)   ──┘        ↑
                    Bitterblossom incident-responder workload ──┘
                            ↑
012 (delivery ledger) ──────┘  load-bearing for agent consumers
003 (non-fatal webhooks) — prerequisite for responder reliability
004 (correlation paths) — prerequisite for responder signal quality
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
039 (external-user security/privacy foundation) — must precede arbitrary-user hosted claims; adds tenant/project ownership, public-ingest constraints, privacy defaults, webhook scoping, and quotas
041 (live integration verification harness) — shipped; strict now proves the production-image SDK/write/readback/webhook/doctor/MCP path before integration coverage claims scale beyond dogfood
042 (runtime pressure/freshness ops) — shipped; worker/job/readiness/backup/dogfood freshness now fails loudly instead of silently aging under arbitrary-app scale
040 (universal integration/enrollment engine) — builds on 039/041/042 to make arbitrary app onboarding state-aware, framework-neutral, and receipt-backed
044 (telemetry/analytics signal model) — defines what analytics/log/metric/event signals Canary owns or bridges before adding broad ingest surfaces
043 (agentic remediation claim protocol) — shipped; typed claims now add deterministic ownership/claim state for downstream triage agents
045 (self-watch operator verdict) — makes Canary's own watchman state one actionable agent-readable verdict, including witness, worker pressure, incidents, dogfood gaps, and next operator action
046 (dogfood value receipts) — turns coverage into per-service value proof: current signal, owner/claim, action, outcome, stale evidence, and verification receipt
047 (alert-plane reliability/SLO burn-rate) — separates route readiness from alerting ability, adds check-in skew safety, and introduces SLO/error-budget feedback after an induced impairment rehearsal proves the alert-plane verdict
048 (responder rich-context safety gate) — narrows responder authority, enforces minimized/audited rich context, defines safe browser/public-ingest authority, and aligns HTTP/CLI/MCP responder privileges before arbitrary-user auto-triage
049 (integration evidence/capture gaps) — closes residual post-040 overclaiming gaps: synthetic service-specific readback, service-specific webhooks, stale-evidence failure, safe browser capture after 048, platform env parity, integrate apply, and MCP wrapper
050 (cold-agent readiness proof) — codifies a one-entrypoint proof that a cold agent can inspect Canary, discover MCP/CLI surfaces, run fast validation, and leave a redacted readiness receipt

022 (contract hygiene) ──── ships independently; restores summary invariant + supervision-tree collapse
023 (incident detail API) ──→ Canary-side substrate for the Bitterblossom responder workload (and thus 010 ramp pattern)
024 (signal-agnostic annotations) ──→ blocked on 023; completes the Ramp-loop writable-metadata primitive
046 ──→ 047 ──→ 048 ──→ Bitterblossom 055 ──→ 049 ──→ 010
047 + 049 ──→ 050 (readiness proof consumes the real alert and MCP/integration surfaces)
```

## Execution Lanes

**Lane 1 (agent readiness):** 012 (delivery ledger) → Bitterblossom incident-responder workload → 010 (ramp)
  · **023 (incident detail API) → 024 (signal-agnostic annotations)** land the Canary-side substrate the responder workload consumes
**Lane 2 (contract + observability):** 011 (OpenAPI) + 013 (metrics) — parallel, no deps · **030 (agent contract safety)** depends on 011 + 012 and tightens the existing contract for autonomous consumers · **031 (agent replay determinism)** shipped the malformed replay/query/health contract errors · **032 (live Rust write-path evidence)** proves the Rust production surface beyond read-only smoke
**Lane 3 (structural):** 006 (query split) → 005 (connect-a-service) · **022 (contract hygiene + shallow-module collapse)** — ship first of the active set; unblocks nothing but restores the summary invariant and sheds ~300 LOC of drift
**Lane 4 (hardening):** 008, 014, 016, 017, 018, 019 (independent, small, can ship anytime) · **034 (worker lifecycle readiness oracle)** hardens the Rust background-worker proof surface
**Lane 5 (dogfood coverage):** 020 (Adminifi HTTP surface verification) · **033 (deployed service registry lifecycle)** shipped the managed registry substrate · **035 (deployed app Canary coverage)** makes every active owned deployment covered or explicitly blocked · **036 (agent-native inspection surface)** gives agents the operating view · **037 (watch the watchmen)** proves Canary externally · **038 (one-command integration agent)** removes setup friction
**Lane 6 (arbitrary-app productization):** 039 (external-user security/privacy foundation) → 041 (live integration verification harness) + 042 (runtime pressure/freshness ops) → 040 (universal integration/enrollment engine) → 043 (agentic remediation claim protocol) + 044 (telemetry/analytics signal model) → **048 (responder rich-context safety gate)** → Bitterblossom **055 (canary/incident responder template)** → **049 (integration evidence/capture gaps)** → 010 (Ramp pattern)
**Lane 7 (product feedback loop):** 045 (self-watch operator verdict) shipped → 046 (dogfood value receipts) shipped → **047 (alert-plane reliability/SLO burn-rate)** — turns Canary dogfooding from coverage checks into value, alertability, and operator-action proof
**Lane 8 (cold-agent readiness):** 050 — after the alert-plane and integration/MCP surfaces harden, package the cold-agent verification path into one discoverable proof and receipt

### Active order (2026-06-17)

045 shipped in PR #163 (commit b0c43b5). 046 shipped the per-service dogfood
value receipt loop. `bin/canary dogfood value --service <name> --json` now
answers what Canary proves for one service, and `doctor --json` includes
aggregate dogfood value counts. 047 is still the best next pickup, but the
first slice is now the induced alert-plane impairment proof: make doctor and
the external witness degrade on alertability failure before implementing
broader SLO/error-budget math.

048 stays pending until the self-watch/value/SLO loops settle, because
arbitrary-user rich-context responders need those contracts to avoid
over-authorizing. 048 now also owns the public-ingest or relay boundary for
browser capture and the HTTP/CLI/MCP authority parity that arbitrary responders
need. After 048, the cross-repo pickup is Bitterblossom
`/Users/phaedrus/Development/bitterblossom/backlog.d/055-workload-template-portfolio.md`
child 2, the canary/incident responder template. 049 follows that responder
template and closes residual integration evidence gaps without redoing shipped
040 enrollment work. 050 should not preempt 047/048/049; it packages the
agent-facing verification proof once the surfaces it proves are real. 020 stays
blocked on Adminifi URLs. 010 stays blocked on that downstream Bitterblossom
responder workload, but the Lane 7 alertability proof, responder-safety
foundation, and cold-agent proof now precede any claim that Canary is ready for
arbitrary external users.

022 + 023 landed on 2026-04-21. 024 landed on 2026-04-22. 026 landed on
2026-04-23 — Ramp
substrate now complete. The downstream Bitterblossom responder workload
unblocks the north star. Elixir-era lint and parity backlog items were retired
during the Rust scorched-earth migration. 010 stays blocked on that
Bitterblossom workload. 020 stays blocked on Adminifi URLs.

## Migration Notes

- Consolidated from `.backlog.d/` on 2026-03-30. Legacy items archived to `.backlog.d/_done/`.
- `.backlog.d/006` (monorepo bootstrap) archived as shipped — commit `c87f28f`.
- `.backlog.d/008` (monitor generation spike) superseded by 010-ramp-pattern.
- The old Bitterblossom triage sprite path `bitterblossom/backlog.d/011-canary-triage-sprite.md`
  is stale; the active blocker is Bitterblossom `055` child 2, the
  canary/incident responder template that uses Canary claims and annotations.
- 2026-04-02: Added 012–015 from multi-AI architecture audit. Promoted 006, 011 to high.
- 2026-04-21: Added 022–024 from grooming investigation (three parallel investigators: archaeologist / strategist / scout). Three themes: contract hygiene, incident-as-atomic-agent-unit, signal-agnostic annotations. 022 ready to ship first; 023 + 024 land the Canary-side substrate for the ramp pattern.
- 2026-05-19: Groomed stale active backlog. Archived 025 as subsumed by #026
  and archived shipped 027. Added 030 from the agent-contract safety theme:
  per-operation scope metadata, summary completeness discipline, cold-start
  guidance, annotation write-back conventions, and delivery-id-addressable
  webhook diagnostics without crossing the responder boundary.
- 2026-05-24: Groomed toward usefulness/elegance: promoted #030, added #031
  for deterministic replay/health/readiness boundary failures, and clarified
  that #010 is now blocked on the downstream Bitterblossom responder workload
  rather than Canary-side annotation/timeline substrate.
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
- 2026-06-13: Mega-groomed the arbitrary-app product direction. Added 039-044:
  external-user security/privacy foundation, live integration verification
  harness, runtime pressure/freshness ops, universal integration/enrollment,
  typed remediation claims, and telemetry/analytics signal modeling. Evidence
  came from live `bin/canary doctor`, `bin/dogfood-inventory --strict`,
  integration discovery false negatives against LineJam/Chrondle/Misty/Vanity,
  source inspection, external exemplar docs, and read-only swarm lanes.
- 2026-06-13: Delivered 039. Canary now has tenant/project/service ownership
  across API keys, ingest, read models, admin surfaces, annotations, incidents,
  health, webhooks, and delivery rows; service-bound keys are fail-closed, server
  ingest applies redaction defaults, webhook delivery uses timestamped signatures
  and scoped service authority, and durable rate limits back process-local
  buckets.
- 2026-06-13: Delivered 041. Dagger strict now runs a production-image
  integration harness that proves health/readiness workers, TypeScript SDK
  ingest/readback against the Rust server, disposable target/monitor/webhook
  write paths, webhook delivery ledger readback, doctor worker readiness, and
  MCP manifest schema shape.
- 2026-06-14: Delivered 042. `/readyz` now derives required-worker readiness
  from freshness, consecutive failures, pressure, and lifecycle state; webhook
  delivery recovers stale executing leases with auditable job errors; doctor
  reports DR/Litestream evidence and restore receipts; startup can fail closed
  on missing Litestream with `CANARY_REQUIRE_LITESTREAM=1`; dogfood evidence
  expires in strict mode; and the server test suite covers single-writer
  contention across ingest, probe scheduling, webhook delivery, and retention
  pruning.
- 2026-06-14: Delivered 040. `bin/canary integrate status` now reconciles local
  scan, receipt, platform env-name evidence, live Canary state, query readback,
  webhook state, and dogfood inventory into a coverage verdict; integration
  patch/enroll writes reviewable receipts; static/Vercel and non-HTTP runtimes
  get concrete coverage paths; and the TypeScript SDK now carries the Next.js,
  Sentry, browser, health, and check-in helpers needed by arbitrary consumers.
- 2026-06-14: Delivered 043. Canary now has durable remediation claims for
  incidents, error groups, targets, and monitors, with typed ownership,
  idempotent create, bounded transitions, TTL expiry, conflict Problem Details,
  query/report/incident/annotation surfacing, lifecycle webhooks, CLI/MCP
  helpers, fail-closed claim row validation, and OpenAPI guidance for agentic
  remediation handoff.
- 2026-06-14: Delivered 044. Canary now has a typed native analytics event
  model with bounded storage, timeline/report correlation, scoped event
  webhooks, TypeScript SDK and CLI/MCP capture helpers, and OpenAPI guidance;
  metrics, logs, and traces remain explicit bridge-only responsibilities until
  an OpenTelemetry integration is designed.
- 2026-06-15: Strategic research/groom for dogfooding, self-watch, product
  feedback, alerting, and auto-triage boundaries. Added 045-049. Live evidence:
  `bin/canary summary --json` reported Canary unhealthy because
  `canary-watchman` was down; `bin/canary doctor --json` showed route readiness
  plus stale witness/check-in and worker pressure; `bin/canary dogfood audit
  --strict --json` found 35 strict failures including unregistered deployments,
  stale registry evidence, and completed-ticket next actions. External research
  supported SLO burn-rate alerting and the Sentry-style split where the
  observability product owns rich context while external agents own code
  mutation. Updated 010 to depend on a Bitterblossom incident-responder workload
  using Canary claims, not the stale archived `bb/011` sprite.
- 2026-06-17: Delivered 045. `bin/canary doctor --json` now emits a `verdict`
  object with `overall`, `blocking_signals`, `next_operator_action`,
  `witness_age_ms`, `open_canary_incident`, `worker_pressure`,
  `dogfood_gap_count`, and `receipt_run_references`. Pressured workers are
  separate from failing workers. MCP manifest covers summary, errors, services,
  incidents, timeline, targets, monitors, doctor, witness, DR status, and
  dogfood audit. Fixture `doctor_watchman_down.json` proves the live
  `readyz ok + witness down + open incident` shape stays actionable. Shipped in
  PR #163 (commit b0c43b5).
- 2026-06-18: Delivered 046. `bin/canary dogfood value --service <name>
  --json` now builds per-service receipts from dogfood coverage plus live
  target/monitor, error, incident, claim, annotation, and telemetry evidence.
  Live pilot receipts distinguish `linejam` as proven from `chrondle` stale
  registry evidence, `doctor --json` surfaces aggregate dogfood value counts,
  and MCP exposes `canary_dogfood_value`.
- 2026-06-19: Strategic groom from the new agent-first vision. Live
  `bin/canary doctor --json` was healthy with fresh witness receipts, but
  `bin/canary dogfood audit --strict --json` still failed on 45 coverage and
  evidence gaps. Swarm lanes converged on three backlog moves: sharpen 047
  around an induced alert-plane impairment proof before burn-rate math, expand
  048 into the arbitrary-responder safety gate including public-ingest/relay
  and authority parity, keep 049 pending behind 048 while hardening its
  anti-overclaim receipt oracle, and add 050 for a cold-agent readiness proof.

## Status

- `ready`: scoped and buildable now
- `blocked`: keep visible, but do not start before the listed dependency lands
- `done`: completed and archived under `_done/`
