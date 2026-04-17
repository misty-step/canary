# Check-in monitors for non-HTTP runtimes

Priority: medium
Status: ready
Estimate: M

## Goal
Implement the canonical non-HTTP health model selected in `009` by adding
check-in monitors for desktop apps, cron jobs, and workers.

## Non-Goals
- Roll the new protocol out across external repos in this item
- Replace HTTP `Target` polling for services that already have stable health URLs
- Overload `POST /api/v1/errors` with liveness semantics

## Oracle
- [ ] Given a non-HTTP monitor is created, when `POST /api/v1/check-ins` receives `alive`, `in_progress`, `ok`, or `error`, then Canary stores the check-in and updates monitor state deterministically
- [ ] Given a monitor misses its configured cadence or TTL, when the state evaluator runs, then Canary emits `health_check.degraded`, `health_check.down`, and `health_check.recovered` timeline/webhook events with monitor payloads
- [ ] Given report and status APIs are queried, when non-HTTP monitors exist, then their state appears without pretending they are URL-backed targets
- [ ] Given an integration wants to emit actual crashes, when it uses `POST /api/v1/errors`, then error telemetry remains separate from check-in health state

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
