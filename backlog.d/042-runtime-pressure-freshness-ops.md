# Make runtime operations pressure- and freshness-aware

Priority: P0
Status: ready
Estimate: L

## Goal
Make Canary operationally boring under arbitrary-app load by tying readiness and operator reports to worker freshness, queue age, retry pressure, probe lag, backup evidence, and dogfood evidence freshness.

## Oracle
- [ ] Webhook delivery jobs cannot strand forever in `executing`; startup or periodic recovery leases stale executing rows back to retry/discard with an auditable reason.
- [ ] `/readyz` fails when required workers are stale, repeatedly failing, blocked behind queue pressure, or missing recent successful passes beyond configured thresholds.
- [ ] Target probes, monitor overdue checks, webhook delivery, retention pruning, and TLS scanning expose due counts, in-flight counts, oldest due age, last success, failure counts, and backoff/circuit state in agent-readable summaries.
- [ ] Production startup and `doctor` surface Litestream/DR status, last successful backup check, and last restore-drill receipt; production can be configured to fail readiness on missing backup guarantees.
- [ ] Dogfood registry evidence expires by policy; strict audit fails stale `last_checked_at`, completed-ticket next actions, and registry entries whose live state no longer matches.
- [ ] Load/pressure tests cover single-writer contention between ingest, probes, webhook delivery, and retention pruning.

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
