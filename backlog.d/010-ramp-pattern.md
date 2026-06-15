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
Blocked on: Bitterblossom's canary/incident responder template, tracked in
`/Users/phaedrus/Development/bitterblossom/backlog.d/055-workload-template-portfolio.md`
and named in `/Users/phaedrus/Development/bitterblossom/project.md` under the
workload roadmap. That workload consumes Canary webhooks, replays
timeline/report state, creates remediation claims, and writes
annotations/evidence back to Canary. The older `bb/011` triage-sprite reference
is stale; that ticket was archived as abandoned in Bitterblossom, and Canary now
has remediation claims that replace the old annotation-lease design.
Reference: "How we made Ramp Sheets self-maintaining" (Ramp Labs, 2026-03-23).
Current state: Canary-side prerequisites for annotations, timeline replay,
incident detail, signal-agnostic write-back, remediation claims, telemetry
events, dogfood inspection, and one-command integration foundations have landed.
Next step: shape and ship the Bitterblossom `055` canary/incident responder
template so the triaging agent can claim work, propose fixes, and verify
outcomes while Canary remains the observability substrate.

Source: spellbook simplification session 2026-03-25.
Refined: grooming session 2026-03-30 — decided annotations over triage state machine.
