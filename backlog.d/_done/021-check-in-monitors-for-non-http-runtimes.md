# Check-in monitors for non-HTTP runtimes

Priority: medium
Status: done
Estimate: M

## Goal
Implement the canonical non-HTTP health model selected in `009` by adding
check-in monitors for desktop apps, cron jobs, and workers.

## Non-Goals
- Roll the new protocol out across external repos in this item
- Replace HTTP `Target` polling for services that already have stable health URLs
- Overload `POST /api/v1/errors` with liveness semantics

## Oracle
- [x] Given a non-HTTP monitor is created, when `POST /api/v1/check-ins` receives `alive`, `in_progress`, `ok`, or `error`, then Canary stores the check-in and updates monitor state deterministically
- [x] Given a monitor misses its configured cadence or TTL, when the state evaluator runs, then Canary emits `health_check.degraded`, `health_check.down`, and `health_check.recovered` timeline/webhook events with monitor payloads
- [x] Given report and status APIs are queried, when non-HTTP monitors exist, then their state appears without pretending they are URL-backed targets
- [x] Given an integration wants to emit actual crashes, when it uses `POST /api/v1/errors`, then error telemetry remains separate from check-in health state

## Notes
Implements the decision captured in
`docs/non-http-health-semantics.md`.

Expected shape:

- new admin-managed non-HTTP monitor entity, separate from `Target`
- new write endpoint: `POST /api/v1/check-ins`
- two monitor modes:
  - `schedule` for cron / recurring jobs
  - `ttl` for session heartbeats / long-running workers
- reuse existing `health_check.*` business event taxonomy for timeline and webhook consumers

## What Was Built

Closed on 2026-04-18.

- Added first-class non-HTTP monitors with admin CRUD at `GET/POST /api/v1/monitors` and `DELETE /api/v1/monitors/:id`.
- Added `POST /api/v1/check-ins`, which records `alive`, `in_progress`, `ok`, and `error` check-ins without overloading error ingest.
- Added deterministic overdue evaluation so missed cadences transition monitor state through degraded/down and recover on the next healthy check-in.
- Reused the existing `health_check.degraded`, `health_check.down`, and `health_check.recovered` event taxonomy for timeline, report, incident correlation, and webhook consumers.
- Exposed monitors directly in health status, unified report, CSV export, and the OpenAPI contract instead of faking URL-backed targets.

## Verification

- `mix test test/canary/monitors_test.exs test/canary_web/controllers/monitor_controller_test.exs test/canary_web/controllers/check_in_controller_test.exs test/canary/status_test.exs test/canary/report_test.exs test/canary_web/controllers/status_controller_test.exs test/canary_web/controllers/report_controller_test.exs test/canary_web/controllers/health_controller_test.exs`
- `mix test test/canary/summary_test.exs`
- `mix test test/canary_web/controllers/openapi_controller_test.exs`
- `./bin/validate --strict`
