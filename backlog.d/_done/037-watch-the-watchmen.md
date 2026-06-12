# Add an independent witness for Canary itself

Priority: high
Status: done
Estimate: L

## Goal
Make Canary's own availability, worker continuity, and error ingest observable even when Canary is the system being checked.

## Oracle
- [x] `canary-self` coverage includes `/healthz`, `/readyz`, and `service=canary` error-query readback, with documented expectations for each signal.
- [x] A small external witness runs outside the `canary-obs` Fly app and checks Canary on a schedule, preserving a receipt outside Canary when Canary is unreachable.
- [x] The witness sends Canary check-ins when Canary is healthy, records latency and status, and exposes a simple failure notification path that does not depend on Canary being reachable.
- [x] The witness has a deterministic local test with fake Canary responses and a production evidence packet showing healthy, degraded, and unreachable handling.
- [x] Canary's agent inspection surface from #036 shows witness status next to self-target, worker readiness from #034, and recent `service=canary` errors.
- [x] `./bin/validate --fast` is green.

## Closure

Delivered `bin/canary-witness`, a GitHub Actions scheduled witness in
`.github/workflows/uptime-monitor.yml`, and `docs/canary-witness.md`. The
witness runs outside the `canary-obs` Fly app, checks `/healthz`, `/readyz`, and
`/api/v1/query?service=canary&window=1h`, writes a JSON receipt, uploads it as a
GitHub Actions artifact, opens/closes a GitHub issue independent of Canary, and
posts a `canary-watchman` check-in only when the self-signals are healthy.

`bin/canary doctor` now includes `witness` and `canary_errors` lines while
preserving the explicit `worker_readiness: unavailable until #034 lands` state.
Deterministic coverage lives in `test/bin/canary_witness_test.sh` with healthy,
degraded, unreachable, and missing check-in credential scenarios.

## Notes
**Why:** Watchmen perspective. A self-target proves the HTTP process can answer from inside Canary, but it does not prove an outside agent can reach Canary or preserve evidence when Canary is down.

**Candidate substrates.** Start with the smallest independently hosted witness: a scheduled GitHub Action, a tiny Vercel cron, or a separate Fly app. The acceptance criterion is independence and durable receipts, not a specific platform.

**Failure rule.** If Canary is unreachable, the witness must not lose the only proof by trying to write exclusively to Canary. It should keep an external receipt and use a simple fallback channel chosen at implementation time.

**Related.** #034 exposes internal worker readiness; this item proves Canary from outside the process.
