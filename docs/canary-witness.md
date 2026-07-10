# Canary Witness

Canary monitors itself from two directions:

- Inside Canary, the `canary-self` HTTP target checks
  `$CANARY_ENDPOINT/healthz`.
- Outside Canary, the scheduled GitHub Actions witness asks to run every five
  minutes and preserves a JSON receipt as a workflow artifact. GitHub cron is
  best-effort, so the production workflow sends a two-hour `ttl_ms` check-in
  to avoid false witness-down incidents when scheduled runs are delayed.

The external witness is intentionally a shell script rather than a Rust service
because the production substrate is GitHub Actions cron. Avoiding a scheduled
Cargo build keeps the witness fast, portable, and independent of the host it is
checking.

## Checked Signals

`bin/canary-witness` verifies three Canary self-signals:

| Signal | Route | Healthy expectation |
|---|---|---|
| Liveness | `GET /healthz` | HTTP 200 and `{"status":"ok"}` |
| Readiness | `GET /readyz` | HTTP 200, `{"status":"ready"}`, database and supervisor `ok`, and all five worker lifecycle snapshots started with zero failures. This is route readiness, not alert-plane health. |
| Alert plane | `/readyz` worker snapshots | Every required worker except `monitor_overdue` has `health: ok`, no backoff/circuit pressure, and no stale due-work pressure. `monitor_overdue` pressure is out of the witness's own scope (see below) and does not by itself degrade the witness. Any other `pressured` worker still keeps route-ready but blocks the witness from reporting healthy. |
| Error readback | `GET /api/v1/query?service=canary&window=1h` | HTTP 200, service `canary`, and numeric `total_errors` |

### Witness scope: `monitor_overdue` never blocks the witness alone

The witness's own health verdict is scoped to its own signals — healthz,
readyz worker health, and the canary-query self-check — not the entire alert
plane. `monitor_overdue` tracks *every* monitor's heartbeat schedule,
including monitors owned by other services (found live in production
2026-07-06: `linejam-production-smoke`, an unrelated monitor, kept this
witness's own GitHub issue open indefinitely even after its actual key fault
was fixed) and this witness's own monitor (normally `canary-watchman`,
found live 2026-07-02 stuck overdue for 11+ hours because its own pressure
had locked out its own recovery check-in). Neither case is in scope for
whether Canary/the witness process itself is healthy:

- An unrelated service's overdue heartbeat must never block this witness —
  the witness has no way to resolve someone else's monitor and should not be
  held hostage to it.
- The witness's own overdue heartbeat must not block its own check-in either,
  since the check-in is the only thing that clears it — refusing it would
  deadlock forever.

So whenever `monitor_overdue` is the *only* impaired worker — regardless of
which monitor(s) triggered it — the witness still reports `status: healthy`
and still sends its check-in. Impairment involving any other worker
(`webhook_delivery`, `retention_prune`, `target_probe`, `tls_scan`) still
blocks the witness exactly as before; this scoping does not weaken the bar
for genuine Canary-process degradation. The receipt's `alert_plane` block
still reports the true, unscoped alert-plane status and impaired workers for
observability, and `self_heal_check_in` records whenever the witness's own
`healthy` verdict or check-in relied on this scoping rather than a fully
clean alert plane.

When all route and alert-plane signals are healthy, the witness sends an ingest check-in:

```json
{
  "monitor": "canary-watchman",
  "status": "alive",
  "summary": "Canary witness saw healthy self-signals and 0 recent canary errors."
}
```

The check-in proves Canary can still ingest from an external observer. The
receipt remains useful when the check-in cannot be delivered. When
`CANARY_WITNESS_TTL_MS` or `--ttl-ms` is set, the check-in includes `ttl_ms`
so Canary uses that TTL for the next deadline on TTL-mode monitors.

## GitHub Schedule

`.github/workflows/uptime-monitor.yml` runs the witness outside the Canary host. On
each run it uploads `canary-witness-receipt.json`. On failure it opens a GitHub
issue labeled `canary-witness-failed`; on recovery it closes the issue. This
notification path does not depend on Canary being reachable.

The workflow keeps the cron expression at `*/5 * * * *` for best-effort
freshness but sets `CANARY_WITNESS_TTL_MS=7200000` because observed GitHub
scheduled runs can land roughly hourly or later. Tightening this TTL requires
moving the witness to a scheduler that can meet the tighter cadence.

Required repository secrets:

- `CANARY_WITNESS_READ_KEY`: read-scoped or admin-scoped key for the query
  readback.
- `CANARY_WITNESS_INGEST_KEY`: ingest-scoped or admin-scoped key for the
  `canary-watchman` check-in.

Forks can leave the witness workflow unconfigured. Upstream Misty Step runs the
workflow against its production instance; forks run it only after setting the
`CANARY_WITNESS_ENDPOINT` repository variable and the two witness secrets.

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
bin/canary errors list canary --window 1h
```

`doctor` reports the external witness next to the self-target/readback view. If
the witness is `missing`, provision the `canary-watchman` monitor and GitHub
secrets. If it is `configured` but not `observed`, inspect the latest GitHub
Actions receipt. If it is `observed`, the check-in state and timestamp are the
current external witness evidence.

`doctor` also summarizes worker lifecycle readiness from `/readyz`, for example
`worker_readiness: ready 5 workers, 0 failing`, and the stricter alert-plane
verdict, for example `alert_plane: healthy 5 workers`. Treat a missing,
failing, or impaired alert-plane line as an operational alertability signal,
not as a deploy-readiness signal.

The witness itself now requires each `/readyz` worker to report `state:
started`, `health: ok`, zero cumulative and consecutive failures, a string
`last_success_at`, numeric pressure counters, and `backoff_or_circuit_open:
false` before the alert plane is healthy. A pressured worker is still
route-ready evidence; it is not healthy alert-plane evidence.

`doctor` also prints a `dr:` line. That line reflects the operator
`bin/dr-status --host "$CANARY_SSH_HOST"` Litestream check when available and points to
the latest checked-in restore-specific receipt when one exists; otherwise it
reports `restore_receipt_missing` and the fallback runbook path.
