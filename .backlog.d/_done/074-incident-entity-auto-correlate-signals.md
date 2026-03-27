# Incident Entity Auto-Correlate Signals

Priority: high
Status: done
Estimate: M

## Goal
Model incidents as first-class correlated objects instead of making callers stitch together health and error streams manually.

## Non-Goals
- Add LLM-based correlation
- Add a dedicated incidents REST API in this item
- Change existing non-incident webhook payloads

## Oracle
- [x] Given health and error signals for the same service, when correlation runs, then one incident with attached signals is produced
- [x] Given incidents are active, when the unified report is generated, then correlated incidents appear in the response
- [x] Given the work shipped, when the codebase is inspected, then `lib/canary/incidents.ex` and `test/canary/incidents_test.exs` are present

## Notes
Migrated from GitHub #74. Already implemented in the current tree and exercised by the incidents and report integration tests.
