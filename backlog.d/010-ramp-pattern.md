# Evolve toward Ramp-pattern autonomous maintenance

Priority: high
Status: blocked
Estimate: XL

## Goal
Canary generates monitors per code change, auto-triages alerts, and proposes fixes — matching the Ramp Sheets self-maintaining pattern.

## Non-Goals
- Replace Datadog/Grafana (instrument, don't own the observability platform)
- Full auto-merge of fixes (human review gate stays)

## Oracle
- [ ] On PR merge, agent reads diff and generates monitors
- [ ] When monitor fires, agent is dispatched with alert context
- [ ] Agent reproduces issue in sandbox, pushes fix PR
- [ ] If alert is noise, agent tunes or deletes the monitor
- [ ] State on monitor prevents duplicate work

## Notes
Blocked on: triage sprite in bitterblossom (`bb/011-canary-triage-sprite.md`).
Reference: "How we made Ramp Sheets self-maintaining" (Ramp Labs, 2026-03-23).
Current state: Canary-side prerequisites for annotations, timeline replay,
incident detail, and signal-agnostic write-back have landed. `#030` and `#031`
make the remaining agent contract and replay boundaries machine-verifiable
before the downstream triage sprite closes the loop.
Next step: ship the bitterblossom triage sprite, then close the loop so the
triaging agent can propose fixes while Canary remains the observability
substrate.

Source: spellbook simplification session 2026-03-25.
Refined: grooming session 2026-03-30 — decided annotations over triage state machine.
