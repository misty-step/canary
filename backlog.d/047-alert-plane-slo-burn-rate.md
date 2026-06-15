# Add alert-plane reliability and SLO burn-rate feedback

Priority: P0 · Status: ready · Estimate: XL

## Goal
Separate deploy readiness from operational alert health by adding SLO/error-budget signals, alert-plane impairment checks, and check-in timestamp safety.

## First Deliverable
Start by defining and exposing alert-plane health separately from route
readiness. The first slice should make sustained worker pressure, stale due
work, circuit-open suppression, or webhook fanout failure visible to
`bin/canary doctor`, the external witness, and a focused regression fixture
before adding SLO configuration or burn-rate math.

## Oracle
- [ ] Given webhook delivery, monitor overdue, target probe, retention, or TLS workers report sustained pressure, circuit-open suppression, stale due work, or fanout failures, when the external witness runs, then it fails or degrades with an alert-plane impairment reason even if `/readyz` is route-ready.
- [ ] Given a monitor check-in contains a future `observed_at` beyond an allowed skew, then Canary rejects or clamps it with RFC 9457 Problem Details and a regression test proves future check-ins cannot defer overdue alerts.
- [ ] Given configured per-service SLO objectives, when `/metrics`, `/api/v1/report`, or `bin/canary services --json` is queried, then Canary exposes windowed availability/error/latency SLIs, budget remaining, burn rate, and recommended alert severity.
- [ ] Given an induced rehearsal creates alert-plane impairment, then a live or production-image rehearsal proves the witness and doctor catch it before declaring Canary healthy.

## Notes
Why: the reliability lane found that `/readyz` intentionally treats `pressured` workers as route-ready, while the witness accepts `ok` or `pressured`. That can be appropriate for deploy readiness, but it is not enough for alerting health. The lane also found no SLO or burn-rate implementation and no check-in timestamp skew policy.

External research supports multi-window burn-rate alerting as the mature SLO alerting shape. Start with coarse service classes and fixed defaults rather than bespoke thresholds per service.

## Children
1. Define alert-plane health separately from route readiness.
2. Add check-in timestamp skew policy and tests.
3. Add windowed SLI read models for targets, monitors, errors, and incidents.
4. Add service SLO configuration with default classes.
5. Add burn-rate summaries to report/CLI/MCP and route notification severity to page vs ticket semantics.
6. Add an induced impairment rehearsal to strict or a dedicated ops gate.
