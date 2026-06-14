# Worker Lifecycle Readiness

Date: 2026-06-12

## Contract

`GET /readyz` now reports one process-local lifecycle snapshot for each
background worker under `checks.workers`:

- `webhook_delivery`
- `target_probe`
- `monitor_overdue`
- `retention_prune`
- `tls_scan`

Each worker snapshot contains:

- `name`: stable worker name.
- `state`: `started` or `stopped`.
- `health`: derived readiness classification: `ok`, `stale`, `failing`,
  `pressured`, or `stopped`.
- `last_success_at`: timestamp for the last successful lifecycle pass, or
  `null` before any pass succeeds.
- `last_success_age_ms`: age of the last successful pass, when the runtime has
  a live clock sample.
- `failure_count`: count of caught runtime errors or panics in the worker loop.
- `consecutive_failures`: consecutive runtime failures since the last
  successful lifecycle pass.
- `last_error_class`: sanitized class such as `runtime_error` or `panic`, never
  a raw error message.
- `due_count`: work items due at the last observed lifecycle pass.
- `in_flight_count`: work items still in flight at the last observed pass.
- `oldest_due_age_ms`: oldest due-work lag, when the worker has a scheduled
  queue model.
- `backoff_or_circuit_open`: whether the last pass saw retry, circuit,
  interruption, or fanout pressure.

`/readyz` remains unauthenticated and returns `not_ready` when the database,
supervisor, or any worker is not ready. Worker health is not just thread
liveness: fast workers become stale after 30 seconds without a successful pass,
daily maintenance workers become stale after 25 hours, three consecutive
failures mark a worker failing, and overdue work beyond the pressure threshold
marks the worker pressured. Worker internals remain hidden behind their
lifecycle modules; route handling only serializes the already-computed
snapshot.

## Design Notes

Worker health is runtime state, not durable product data. Keeping it in memory
avoids extra SQLite writes and preserves the single-writer invariant. The store
continues to own product events, check results, delivery ledgers, retention
effects, and timeline data.

The health recorder is shared by:

- webhook delivery drain
- target probe lifecycle
- monitor overdue lifecycle
- retention prune lifecycle
- TLS expiry scan lifecycle

The production image smoke in Dagger now checks `/healthz`, `/readyz`, and the
full worker snapshot array before considering the image ready.
