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
- `last_success_at`: timestamp for the last successful lifecycle pass, or
  `null` before any pass succeeds.
- `failure_count`: count of caught runtime errors or panics in the worker loop.
- `last_error_class`: sanitized class such as `runtime_error` or `panic`, never
  a raw error message.

`/readyz` remains unauthenticated and returns `not_ready` when the database,
supervisor, or any worker is not ready. Worker internals remain hidden behind
their lifecycle modules; route handling only serializes the already-computed
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
