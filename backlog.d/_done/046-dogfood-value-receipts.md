# Prove Canary value with per-service dogfood receipts

Priority: P0 · Status: done · Estimate: L

## Goal
Make every dogfooded service produce a current, machine-readable value receipt that shows what Canary covers, what Canary detected, who or what acted, and what readback proved.

## Oracle
- [x] Given a registered service, when `bin/canary dogfood value --service <name> --json` runs, then it returns coverage verdict, health state, error and incident counts, active remediation claim, recent annotations, recent telemetry events, witness or synthetic verification status, and one next action.
- [x] Given `linejam` and `chrondle` are used as pilots, then the receipts distinguish a reference integration from an integration with current work, and include the last verified outcome or an explicit absence.
- [x] Given a registry entry references completed backlog items such as `035` or `038`, then strict dogfood fails with a repair action that updates the registry language instead of preserving stale next actions.
- [x] Given `bin/canary doctor --json` runs, then it summarizes dogfood value coverage with counts for covered, stale, blocked, partial, and value-unproven services.

## Notes
Why: the current dogfood contract covers inventory, health, ingest, readback, agent affordance, and verification, but it does not yet prove value. During the groom, `bin/canary dogfood audit --strict --json` failed with 35 strict failures, including unregistered deployments, stale registry evidence, and completed-ticket next actions. The product lane recommended a value receipt over a human dashboard.

Value should be measured by useful outcomes: detection-to-context latency, claim-to-fix latency, percent of incidents with active owner, percent of fixes verified through readback, false/noise dismissals, and services with stale coverage evidence.

## Children
1. Define the `DogfoodValueReceipt` schema and text rendering.
2. Attach claim, annotation, event, incident, `/api/v1/status` target/monitor, and integration evidence into the receipt.
3. Add strict registry checks for stale completed-ticket next actions and stale evidence age.
4. Pilot receipts for `linejam`, `chrondle`, `canary-self`, and `misty-step`.
5. Update `doctor` and MCP manifest to surface the value receipt summary.

## Closure
Shipped in PR #166 (2026-06-18). `bin/canary dogfood value --service <name>
--json` now builds a per-service value receipt from dogfood inventory plus live
`/api/v1/status` target/monitor, query, incident, claim, annotation, and
telemetry evidence.
Live pilot receipts prove `linejam` as `value_state=proven` and `chrondle` as
`value_state=stale_registry_evidence` because current 24h readback is clean
while the registry still carries the old `TypeError` triage action.
`bin/canary doctor --json` now includes `response.dogfood_value` counts, and
the MCP manifest exposes `canary_dogfood_value`. Completed-ticket next-action
strict checks were already present in `bin/dogfood-inventory` and
`bin/dogfood-audit`; live strict output reports `sploot` and `misty-step` for
that policy.
