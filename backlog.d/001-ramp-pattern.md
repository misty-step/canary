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
Blocked on: Cerberus CLI (needed for the fix-review loop).
Reference: "How we made Ramp Sheets self-maintaining" (Ramp Labs, 2026-03-23).
Current state: canary-watch synthesizes incidents into GitHub issues.
Next step: close the loop so the triaging agent also proposes fixes.

Source: spellbook simplification session 2026-03-25.
