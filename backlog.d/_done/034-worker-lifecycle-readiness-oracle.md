# Make worker lifecycle readiness observable

Priority: high
Status: done
Estimate: L

## Goal
Expose and gate the health of Canary's background workers so agents can tell whether webhook delivery, target probing, monitor overdue evaluation, retention pruning, and TLS scanning are actually running after boot.

## Oracle
- [x] `/readyz`, metrics, or a narrow read/admin endpoint reports one health snapshot for each lifecycle worker: started/stopped, last successful pass, failure count, and last error class without exposing secrets.
- [x] Webhook delivery, target probe, monitor overdue, retention prune, and TLS scan worker tests assert that panics or runtime errors increment visible counters rather than disappearing into a catch-unwind boundary.
- [x] The production image smoke in `dagger/src/index.ts` verifies `/healthz`, `/readyz`, and at least one worker-backed health signal or worker-health response.
- [x] The implementation keeps worker internals behind deep modules; route handlers translate already-computed health snapshots rather than probing worker state ad hoc.
- [x] `./bin/validate --fast` is green, and `./bin/validate` exercises the production image smoke.

## Notes
**Why:** Runtime/harness perspective. `CanaryServer::boot` starts five dedicated workers, but public readiness currently reports only database and supervisor status, and the Dagger image smoke checks endpoint readiness rather than worker continuity. Catch-unwind boundaries prevent permanent worker death, but agents still need a visible signal when a worker is unhealthy.

**Children**
1. Add a shared worker-health snapshot type and expose it from each lifecycle worker without leaking implementation details.
2. Surface the snapshot through the smallest appropriate wire contract and update OpenAPI if the surface is public.
3. Extend Dagger smoke to assert worker readiness/effects on the production image.

**Responder-boundary check.** This is Canary self-observability only. It does not prescribe how downstream consumers react to degraded worker health.

## Delivered

2026-06-13: `/readyz` now includes process-local lifecycle snapshots for
`webhook_delivery`, `target_probe`, `monitor_overdue`, `retention_prune`, and
`tls_scan`. Each worker loop records started/stopped state, last successful
pass, failure count, and sanitized last error class. Dagger production smoke
asserts all five worker snapshots are present and started.
