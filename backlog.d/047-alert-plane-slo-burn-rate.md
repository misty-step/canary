# Prove alert-plane health separately from route readiness

Priority: P0 · Status: ready · Estimate: XL

## PRD Summary
- User: agent responders and operators deciding whether Canary can be trusted to wake them up.
- Problem: `/readyz` can be route-ready while alert delivery, overdue monitor evaluation, or probe workers are pressured, suppressed, or stale.
- Goal: make Canary distinguish "can serve HTTP" from "can reliably detect and route production health changes."
- Why now: dogfood value receipts prove service health, but the next reliability risk is missing or suppressing the signal that should wake an agent.
- UX enabled: `bin/canary doctor`, the external witness, and strict rehearsal name alert-plane impairment as a first-class reason before claiming Canary is healthy.
- Deliverable type: working code plus production-image rehearsal evidence.
- Success signal: an induced alert-plane impairment fails or degrades the witness/doctor even when `/readyz` remains route-ready.

## Product Requirements
- P0: alert-plane health is a separate verdict from route readiness.
- P0: sustained worker pressure, stale due work, circuit-open suppression, and webhook fanout failure produce stable impairment reasons.
- P0: monitor check-ins cannot use a future `observed_at` timestamp to defer overdue alerts beyond an explicit skew policy.
- P0: the first implementation slice includes an induced impairment fixture before SLO or burn-rate math.
- P1: expose coarse service SLO classes and multi-window burn-rate summaries after the impairment contract is proven.
- Non-goals: replacing `/readyz`, broad distributed tracing, or adding a human dashboard.

## Technical Design
- Chosen architecture: keep `/readyz` as deploy/readiness truth, add an alert-plane verdict derived from worker lifecycle snapshots, webhook delivery reports, monitor overdue reports, target probe reports, and witness readback.
- Files/systems touched: `crates/canary-server/src/worker_health.rs`, worker modules, `crates/canary-cli/src/lib.rs`, `bin/canary-witness`, `test/bin/canary_witness_test.sh`, Dagger production smoke, and focused server tests.
- Data/control flow: workers emit pressure details; doctor/witness grade those into alert-plane impairment; strict or a dedicated rehearsal induces impairment and asserts the degraded verdict.
- Build/check boundary: unit tests cover policy math and timestamp skew; shell tests cover witness status; production-image rehearsal proves the end-to-end route.
- ADR decision: not required for the first slice because this extends the existing readiness/witness architecture; require an ADR if SLO storage introduces a persistent configuration model.
- ADR-style invariants: route readiness stays available for deploy checks; alert-plane readiness is stricter for operational trust; no LLM participates in verdict generation.
- Design X vs Y: do not make `/readyz` fail on every pressure case, because that would conflate deploy safety and alert reliability; instead add a separate stricter alert-plane verdict.

## Goal
Separate deploy readiness from operational alert health by adding SLO/error-budget signals, alert-plane impairment checks, and check-in timestamp safety.

## First Deliverable
Start by defining and exposing alert-plane health separately from route
readiness. The first slice should make sustained worker pressure, stale due
work, circuit-open suppression, or webhook fanout failure visible to
`bin/canary doctor`, the external witness, and a focused regression fixture
before adding SLO configuration or burn-rate math. Include an induced
impairment driver that can be run in strict or as a dedicated ops gate.

## Oracle
- [ ] Given webhook delivery, monitor overdue, target probe, retention, or TLS workers report sustained pressure, circuit-open suppression, stale due work, or fanout failures, when the external witness runs, then it fails or degrades with an alert-plane impairment reason even if `/readyz` is route-ready.
- [ ] Given a monitor check-in contains a future `observed_at` beyond an allowed skew, then Canary rejects or clamps it with RFC 9457 Problem Details and a regression test proves future check-ins cannot defer overdue alerts.
- [ ] Given configured per-service SLO objectives, when `/metrics`, `/api/v1/report`, or `bin/canary services --json` is queried, then Canary exposes windowed availability/error/latency SLIs, budget remaining, burn rate, and recommended alert severity.
- [ ] Given an induced rehearsal creates alert-plane impairment, then a live or production-image rehearsal proves the witness and doctor catch it before declaring Canary healthy.

## Verification System
- Claim: Canary can be route-ready while still refusing to claim alert-plane health.
- Falsifier: a fixture where `/readyz` is ready but webhook delivery is circuit-open, monitor overdue work is stale, or a worker reports sustained pressure and the witness still exits healthy.
- Driver: focused Rust tests, `test/bin/canary_witness_test.sh`, and a production-image induced impairment rehearsal.
- Grader: doctor/witness JSON includes stable alert-plane impairment reasons and non-healthy status; future check-in timestamps beyond allowed skew produce RFC 9457 Problem Details.
- Evidence packet: witness receipt JSON plus Dagger or rehearsal transcript linked from the implementation PR.
- Cadence: unit/shell tests on every gate; induced production-image rehearsal before merge and after alert-plane policy changes.

## Notes
Why: the reliability lane found that `/readyz` intentionally treats `pressured` workers as route-ready, while the witness accepts `ok` or `pressured`. That can be appropriate for deploy readiness, but it is not enough for alerting health. The lane also found no SLO or burn-rate implementation and no check-in timestamp skew policy.

External research supports multi-window burn-rate alerting as the mature SLO alerting shape. Start with coarse service classes and fixed defaults rather than bespoke thresholds per service.

## Children
1. Define alert-plane health separately from route readiness.
2. Add check-in timestamp skew policy and tests.
3. Add an induced impairment rehearsal to strict or a dedicated ops gate.
4. Add windowed SLI read models for targets, monitors, errors, and incidents.
5. Add service SLO configuration with default classes.
6. Add burn-rate summaries to report/CLI/MCP and route notification severity to page vs ticket semantics.
