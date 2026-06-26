# Prove alert-plane health separately from route readiness

Priority: P0 · Status: done · Estimate: XL

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
- P1: expose coarse service SLO classes (shipped) and a low-cardinality SLI **trajectory** signal (delta vs the prior equal-length window) on the report and CLI. Do **not** compute a server-side MWMBR burn-rate ratio against an absolute budget, and do **not** emit a page/ticket severity verdict — see the 2026-06-25 reshape note.
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
- [x] Given webhook delivery, monitor overdue, target probe, retention, or TLS workers report sustained pressure, circuit-open suppression, stale due work, or fanout failures, when the external witness runs, then it fails or degrades with an alert-plane impairment reason even if `/readyz` is route-ready.
- [x] Given a monitor check-in contains a future `observed_at` beyond an allowed skew, then Canary rejects or clamps it with RFC 9457 Problem Details and a regression test proves future check-ins cannot defer overdue alerts.
- [x] Given a service with checks/check-ins/errors over a window, when `/api/v1/report?window=W` and `bin/canary summary --json` are queried, then each service SLI carries the windowed availability/latency/error/incident facts **plus** a `trajectory` block holding the signed delta vs the prior equal-length window, with sub-floor windows marked `insufficient_samples` (null delta) so a single failed probe cannot fake a swing.
- [x] Given the change, when the surfaces are inspected, then **no** server-computed severity verdict and **no** `page`/`ticket` field is emitted on report, CLI, webhook payload, or manifest, and webhook fanout stays severity-agnostic (negative oracle); the regenerated `priv/mcp/canary-cli-tools.json` matches the generator (parity test green) and the runnable MCP server stays deferred to ticket 052.
- [x] Given an induced rehearsal creates alert-plane impairment, then a live or production-image rehearsal proves the witness and doctor catch it before declaring Canary healthy.

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

2026-06-20 slice: first alert-plane impairment proof is implemented on branch
`docs/agent-first-vision`. `bin/canary doctor --json` now exposes
`response.alert_plane`, the doctor verdict carries the same field, and
`bin/canary-witness` exits nonzero with `alert_plane.status: "impaired"` when
the induced fixture returns a route-ready `/readyz` response with a pressured
worker. Evidence:
`docs/architecture/canary-alert-plane-evidence-2026-06-20.md`. Remaining scope:
future check-in timestamp skew policy, production-image induced impairment
gate wiring beyond the shell fixture, SLO configuration, and burn-rate
summaries.

2026-06-23 slice: children 2–5 have shipped on branch
`deliver/047-alert-plane-reliability`. Future check-in timestamp skew
(`a5bb058`), production-image induced impairment rehearsal (`063dece`),
windowed service SLI read models (`51751e1`), and deterministic default SLO
classes (`6bde51d`) are committed. A stray `expect_used`/`expect_err` clippy
debt the unpushed branch had accumulated in tests was also cleared (`5458e51`),
so `cargo clippy --workspace --all-targets -- -D warnings` is clean under the
repo-pinned 1.94 toolchain. Remaining scope: child #6 only — multi-window
burn-rate summaries across report/CLI/MCP and page-vs-ticket notification
severity routing.

2026-06-25 reshape (child #6 only — research + 6-model council vs VISION.md):
the original "burn-rate summaries + page/ticket severity routing" framing was
challenged and narrowed. Research confirmed Google's multi-window/multi-burn-rate
(MWMBR) table assumes high-volume request streams (N≈10^6/h); Canary's data is
low-volume probe/check-in cadence (N≈60-120/h/target), exactly the case the SRE
Workbook warns flaps under literal 5m/14.4x paging. A council of six decorrelated
model families (Moonshot/DeepSeek/Qwen/GLM/MiniMax/xAI) independently rejected a
server-computed page/ticket severity verdict as human on-call vocabulary that
violates Canary's own tenets ("write back evidence, not commands"; "no LLM on the
request path"; "bound the first answer"). That converges with the global
model-native doctrine (do not over-structure interfaces an LLM consumes).
Reshaped scope:
- Expose the windowed SLIs (already serialized on `/api/v1/report`) on the CLI,
  which currently drops the `service_sli` block.
- Add a deterministic, low-N-safe **trajectory** signal per SLI window: the
  signed delta vs the prior equal-length window (e.g. this 1h vs the previous
  1h), with a minimum-sample floor that nulls sub-floor windows as
  `insufficient_samples`. A delta is a fact (evidence); a burn-rate-vs-budget
  ratio is policy the responder should own.
- Keep the two budget notions **separate raw signals** (availability ratio
  trajectory + raw error-count trajectory). Do not synthesize a single
  burn-rate number; drop the `error_budget_events_per_hour` "budget burn"
  framing (events/hour is too jumpy at this cadence; it conflates the probe and
  error-ingest data streams).
- Defer the runnable MCP server to ticket 052; regenerate the manifest snapshot
  so the generated/checked-in parity test stays green and reflects the new CLI
  fields.
- No new query windows (no 5m/30m/3d); no severity verdict; no notification
  routing; webhook fanout stays severity-agnostic.
Full context packet + alternatives + verification:
`docs/architecture/canary-047-child6-sli-trajectory-shape-2026-06-25.html`.

## Children
1. Define alert-plane health separately from route readiness.
2. Add check-in timestamp skew policy and tests.
3. Add an induced impairment rehearsal to strict or a dedicated ops gate.
4. Add windowed SLI read models for targets, monitors, errors, and incidents.
5. Add service SLO configuration with default classes.
6. Surface windowed SLIs + SLO targets + a low-N-safe **trajectory delta** (vs the prior equal-length window) on report/CLI; regenerate the MCP manifest snapshot. **No** server-computed burn-rate ratio, **no** page/ticket severity verdict (reshaped 2026-06-25 — see Notes).

## Closure
Archived on 2026-06-26 after the final child shipped in PR #172
(`467ea85`, `feat(report): add prior-window SLI trajectory to report, CLI, and
OpenAPI`). Earlier slices shipped alert-plane health separate from route
readiness in PR #167 (`c322168`) and check-in skew safety, production-image
induced impairment rehearsal, windowed service SLIs, and default SLO classes in
PR #168 (`ab348e1`). The child #6 reshape landed in PR #171 (`42ffd8f`).

The final implementation exposes per-service windowed SLI summaries plus a
prior-window `trajectory` block on report/CLI/OpenAPI, preserves the negative
oracle of no burn-rate ratio and no page/ticket severity verdict, and keeps the
runnable MCP server deferred to #052.

Residuals deliberately filed as follow-up backlog instead of keeping 047 open:
#056 tracks report lock/owner-scope store debt from review, #057 tracks stale
static MCP manifest snapshot parity, and #058 tracks the cadence-aware
trajectory sample floor.
