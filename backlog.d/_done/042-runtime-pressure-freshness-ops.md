# Make runtime operations pressure- and freshness-aware

Priority: P0
Status: done
Estimate: L

## Goal
Make Canary operationally boring under arbitrary-app load by tying readiness and operator reports to worker freshness, queue age, retry pressure, probe lag, backup evidence, and dogfood evidence freshness.

## Oracle
- [x] Webhook delivery jobs cannot strand forever in `executing`; periodic recovery leases stale executing rows back to retry/discard with an auditable reason.
- [x] `/readyz` fails when required workers are stale, repeatedly failing, blocked behind queue pressure, or missing recent successful passes beyond configured thresholds.
- [x] Target probes, monitor overdue checks, webhook delivery, retention pruning, and TLS scanning expose due counts, in-flight counts, oldest due age, last success, failure counts, and backoff/circuit state in agent-readable summaries.
- [x] Production startup and `doctor` surface Litestream/DR status and restore-drill evidence; production can be configured to fail closed on missing backup guarantees.
- [x] Dogfood registry evidence expires by policy; strict audit fails stale `last_checked_at`, completed-ticket next actions, and registry entries whose live state no longer matches.
- [x] Load/pressure tests cover single-writer contention between ingest, probes, webhook delivery, and retention pruning.

## Children
1. Add webhook executing-job lease recovery and boot recovery tests.
2. Promote worker readiness from lifecycle-state checks to freshness/pressure thresholds.
3. Add queue/probe/retention/TLS pressure read models and CLI/doctor summaries.
4. Integrate DR/Litestream evidence into doctor/readiness policy.
5. Add dogfood evidence expiry and completed-ticket stale-next-action checks.
6. Add contention/load tests around the single SQLite writer and worker loops.

## Notes
- Evidence: `crates/canary-store/src/oban_jobs.rs` claims jobs by moving due rows to `executing`; completion is separate. `crates/canary-http/src/public.rs` currently marks workers ready when lifecycle state is `Started`; `crates/canary-server/src/worker_health.rs` records failure counters but readiness does not threshold them.
- Runtime lane found this is the missing layer between shipped worker visibility and arbitrary-app reliability.

## Completion

Delivered on 2026-06-14.

- Webhook delivery now recovers stale `executing` leases before claiming due
  jobs and records `stale_executing_recovered` in the job error ledger.
- Worker readiness now derives `health` from started state, freshness,
  consecutive failures, and pressure thresholds. `/readyz`, witness checks, and
  production-image smoke require `health: ok`.
- Lifecycle workers report pressure summaries for webhook delivery, target
  probes, monitor overdue evaluation, retention pruning, and TLS scanning.
- `bin/canary doctor` prints DR evidence from `bin/dr-status --app canary-obs`
  and the latest restore receipt. Startup can fail closed with
  `CANARY_REQUIRE_LITESTREAM=1`; `/readyz` remains a local process readiness
  route.
- Dogfood inventory/audit strict mode rejects stale evidence and next actions
  pointing at completed backlog items.

Evidence:

- `docs/architecture/runtime-pressure-freshness-ops-2026-06-14.md`
- `cargo test -p canary-server shared_store_survives_concurrent_runtime_pressure --locked`
- `cargo test -p canary-store webhook_delivery_jobs_recover_stale_executing_leases --locked`
- `cargo test -p canary-server worker_health --locked`
- `cargo test -p canary-cli doctor_summary_includes_watchman_and_self_errors --locked`
- `bash test/bin/dogfood_inventory_test.sh`
- `bash test/bin/dogfood_audit_test.sh`
- `bash test/bin/canary_witness_test.sh`
- `bash test/bin/entrypoint_test.sh`
