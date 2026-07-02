# Unified Report Endpoint

Priority: high
Status: done
Estimate: S

## Goal
Give agents one authenticated API call that answers "what's wrong?" with bounded health, error, and transition data.

## Non-Goals
- Remove or break existing endpoints
- Add LLM processing on the request path
- Replace incidents or search work that builds on top of the report

## Oracle
- [x] Given a report request, when `GET /api/v1/report` is called, then the response returns the bounded agent-facing payload
- [x] Given the report endpoint exists, when the report tests run, then health, error, and pagination behavior are covered
- [x] Given the work shipped, when the codebase is inspected, then `lib/canary/report.ex` and the report controller tests are present

## Notes
Migrated from GitHub #73. Already implemented in the current tree (`lib/canary/report.ex`, `test/canary/report_test.exs`, `test/canary_web/controllers/report_controller_test.exs`).
