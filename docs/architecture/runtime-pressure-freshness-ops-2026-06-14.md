# Runtime Pressure and Freshness Ops

Date: 2026-06-14

## Contract

Canary readiness now treats background-worker health as an operational signal,
not just thread liveness. `GET /readyz` fails when any required worker is
stopped, stale, or repeatedly failing. A `pressured` worker can remain
route-ready, but doctor and the external witness treat it as impaired
alert-plane health. Each worker snapshot exposes the values an agent needs to
decide whether the runtime is keeping up:

- `health`: `ok`, `stale`, `failing`, `pressured`, or `stopped`.
- `last_success_at` and `last_success_age_ms`.
- `failure_count` and `consecutive_failures`.
- `due_count`, `in_flight_count`, and `oldest_due_age_ms`.
- `backoff_or_circuit_open`.

Webhook delivery recovers stale `executing` jobs before claiming new due work.
Recovered rows are either moved back to `scheduled` or discarded when exhausted,
and the row's Oban-compatible `errors` array records
`stale_executing_recovered` with the recovery timestamp and cutoff.
Claim completion is lease-guarded by the claimed row's `attempt` and
`attempted_at`, so a stale executor cannot overwrite a row already recovered by
the scheduler. Webhook pressure reports the due backlog observed before the
claim limit is applied, including `oldest_due_age_ms`, instead of only reporting
the number of rows claimed in one pass.

`bin/canary doctor` now includes DR evidence. It runs
`bin/dr-status --app canary-obs` when available and reports the latest
checked-in restore-specific receipt. If no receipt is found, text output says
`restore_receipt_missing` rather than presenting unrelated evidence or the
fallback runbook as verified restore evidence. Production startup can be
configured with `CANARY_REQUIRE_LITESTREAM=1` to fail closed when the database
is missing and Litestream is unavailable. `/readyz` intentionally remains a
local request-path health check rather than a Fly/Litestream shell-out.

Dogfood evidence is now freshness-gated. Strict dogfood inventory and audit
fail stale or future-dated registry evidence and completed-ticket next actions,
including plain `ticket NNN` references. JSON reports preserve the exact policy
failures for agents.

## Verification

Focused tests:

```bash
cargo test -p canary-server shared_store_survives_concurrent_runtime_pressure --locked
cargo test -p canary-store webhook_delivery_completion_rejects_lost_execution_lease --locked
cargo test -p canary-store webhook_delivery_jobs_recover_stale_executing_leases --locked
cargo test -p canary-server worker_health --locked
cargo test -p canary-server webhook_delivery_drain --locked
cargo test -p canary-cli doctor_summary --locked
bash test/bin/dogfood_inventory_test.sh
bash test/bin/dogfood_audit_test.sh
bash test/bin/canary_witness_test.sh
bash test/bin/entrypoint_test.sh
cd dagger && npx tsc --noEmit
```

The contention oracle is
`shared_store_survives_concurrent_runtime_pressure`: it drives one shared
`Arc<Mutex<Store>>` through concurrent ingest commits, webhook claiming and
stale recovery, active target schedule reads, and retention pruning. The test
fails if the single-writer lock is poisoned or any runtime pressure lane cannot
complete through the public store/lifecycle APIs.

Full gate evidence should be recorded on the branch closeout with:

```bash
PATH=/Users/phaedrus/.local/share/canary-dagger/v0.20.5/bin:$PATH ./bin/validate --fast
PATH=/Users/phaedrus/.local/share/canary-dagger/v0.20.5/bin:$PATH ./bin/dagger call production-image-smoke
PATH=/Users/phaedrus/.local/share/canary-dagger/v0.20.5/bin:$PATH ./bin/validate --strict
```
