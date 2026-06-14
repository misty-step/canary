# Canary Witness

Canary monitors itself from two directions:

- Inside Canary, the `canary-self` HTTP target checks
  `https://canary-obs.fly.dev/healthz`.
- Outside Canary, the scheduled GitHub Actions witness runs
  `bin/canary-witness` every five minutes and preserves a JSON receipt as a
  workflow artifact.

The external witness is intentionally a shell script rather than a Rust service
because the production substrate is GitHub Actions cron. Avoiding a scheduled
Cargo build keeps the witness fast, portable, and independent of the Fly app it
is checking.

## Checked Signals

`bin/canary-witness` verifies three Canary self-signals:

| Signal | Route | Healthy expectation |
|---|---|---|
| Liveness | `GET /healthz` | HTTP 200 and `{"status":"ok"}` |
| Readiness | `GET /readyz` | HTTP 200, `{"status":"ready"}`, database and supervisor `ok`, and all five worker lifecycle snapshots started with zero failures |
| Error readback | `GET /api/v1/query?service=canary&window=1h` | HTTP 200, service `canary`, and numeric `total_errors` |

When all three signals are healthy, the witness sends an ingest check-in:

```json
{
  "monitor": "canary-watchman",
  "status": "alive",
  "summary": "Canary witness saw healthy self-signals and 0 recent canary errors."
}
```

The check-in proves Canary can still ingest from an external observer. The
receipt remains useful when the check-in cannot be delivered.

## GitHub Schedule

`.github/workflows/uptime-monitor.yml` runs the witness outside the Fly app. On
each run it uploads `canary-witness-receipt.json`. On failure it opens a GitHub
issue labeled `canary-witness-failed`; on recovery it closes the issue. This
notification path does not depend on Canary being reachable.

Required repository secrets:

- `CANARY_WITNESS_READ_KEY`: read-scoped or admin-scoped key for the query
  readback.
- `CANARY_WITNESS_INGEST_KEY`: ingest-scoped or admin-scoped key for the
  `canary-watchman` check-in.

Production monitor configuration:

```bash
curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/monitors" \
  -H "Authorization: Bearer $CANARY_ADMIN_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "canary-watchman",
    "service": "canary",
    "mode": "ttl",
    "expected_every_ms": 600000,
    "grace_ms": 120000
  }'
```

## Agent Inspection

Agents should start with:

```bash
bin/canary doctor
bin/canary errors canary --window 1h
```

`doctor` reports the external witness next to the self-target/readback view. If
the witness is `missing`, provision the `canary-watchman` monitor and GitHub
secrets. If it is `configured` but not `observed`, inspect the latest GitHub
Actions receipt. If it is `observed`, the check-in state and timestamp are the
current external witness evidence.

`doctor` also summarizes worker lifecycle readiness from `/readyz`, for example
`worker_readiness: ready 5 workers, 0 failing`. Treat a missing or failing
worker readiness line as a stale inspection surface or a runtime pressure
signal, not as a healthy witness result.

The witness itself now requires each `/readyz` worker to report `state:
started`, `health: ok`, zero cumulative and consecutive failures, a string
`last_success_at`, numeric pressure counters, and a boolean
`backoff_or_circuit_open`. A worker thread that is alive but stale, repeatedly
failing, or pressured is therefore an unhealthy witness result.

`doctor` also prints a `dr:` line. That line reflects the operator
`bin/dr-status --app canary-obs` Litestream check when available and points to
the latest checked-in restore-specific receipt when one exists; otherwise it
reports `restore_receipt_missing` and the fallback runbook path.
