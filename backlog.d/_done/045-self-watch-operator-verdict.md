# Make Canary self-watch produce one actionable operator verdict

Priority: P0 · Status: done · Estimate: L

## Goal
Turn `bin/canary doctor` into the agent control-tower verdict for Canary itself, so a cold agent can see whether Canary is healthy, degraded, or unable to watch itself and what to do next.

## Oracle
- [ ] Given `/healthz` and `/readyz` are healthy but `canary-watchman` is down, when `bin/canary doctor --json` runs against production or a fixture, then it returns `overall=degraded`, includes the stale witness age, links or identifiers for the latest witness receipt/run, names the open Canary incident, and emits a concrete next operator action.
- [ ] Given any worker reports `pressured`, when `doctor` renders text and JSON, then route readiness and operational pressure are reported with consistent semantics rather than counted as both ready and failing without explanation.
- [ ] Given `bin/canary mcp-manifest` is inspected, then services, incidents, timeline, targets, monitors, dogfood, witness, and DR drill-down tools are exposed or explicitly marked out of scope with a replacement command.
- [ ] Given the current live shape `readyz ok + witness down + open canary incident`, then a checked-in fixture test proves the verdict stays actionable.

## Notes
Why: live grooming on 2026-06-15 found Canary self-watch doing the right thing but explaining it poorly. `bin/canary summary --json` returned `status=unhealthy` because `canary-watchman` was down, while `/readyz` was still `ready` and `doctor` reported one pressured worker. The operator lane also found MCP coverage lagging the CLI surface.

Keep the browser dashboard dead. The output surface should be CLI JSON/text and MCP over the same contract.

## Children
1. Normalize the status vocabulary across `doctor`, witness, and worker readiness: `ready`, `pressured`, `degraded`, `down`, and `unknown` must have one meaning.
2. Add a `doctor.verdict` object with `overall`, `blocking_signals`, `next_operator_action`, `witness_age_ms`, `open_canary_incident`, `worker_pressure`, `dogfood_gap_count`, and receipt/run references.
3. Add witness receipt/run discovery for GitHub Actions artifacts without requiring a human dashboard.
4. Expand the MCP manifest to cover the inspection commands agents need for the same drill-downs.
5. Add fixture tests for the live failure shape observed during this groom.

## Closure
Shipped in PR #163 (commit b0c43b5, 2026-06-15). `bin/canary doctor --json`
now emits a `verdict` object with `overall`, `blocking_signals`,
`next_operator_action`, `witness_age_ms`, `open_canary_incident`,
`worker_pressure`, `dogfood_gap_count`, and `receipt_run_references`.
Pressured workers are reported separately from failing workers. MCP manifest
covers summary, errors, services, incidents, timeline, targets, monitors,
doctor, witness, DR status, and dogfood audit. Fixture
`doctor_watchman_down.json` + test `doctor_watchman_down_fixture_stays_actionable`
prove the live failure shape stays actionable.
