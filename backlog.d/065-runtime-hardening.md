# Remove runtime cliffs before broader responder load

Priority: P1 · Status: pending · Estimate: L

## Goal
Harden the hot paths and operational surfaces that will matter once Canary is
used by the full Factory fleet and automated responders: API-key bcrypt outside
the global store mutex, structured tracing, fail-closed backup posture, and
curated security/dependency followups.

## Oracle
- [ ] Given an authenticated ingest burst, then bcrypt verification does not
      hold the single `Store` mutex or starve workers.
- [ ] Given `/api/v1/report` is hot, then store-lock behavior is measured and
      improved without violating the single-writer invariant.
- [ ] Given production boots without Litestream secrets while backup is
      required, then startup fails closed or an explicit alert path fires.
- [ ] Given operators inspect logs, then structured `tracing` output replaces
      ad hoc `eprintln!` on operational paths.
- [ ] Given `RUSTSEC-2026-0190` has a fixed `anyhow`, then the advisory is
      cleared by a narrow dependency update.
- [ ] Given low-frequency targets or monitors have complete windows, then SLI
      trajectory sample floors are cadence-aware.

## Verification System
- Claim: Canary's reliability floor holds under broader fleet and responder
  load.
- Falsifier: API-key verification blocks the writer lock, report reads create
  measurable head-of-line blocking, backup silently disables, or advisories
  become permanent background noise.
- Driver: lock/latency benchmark or targeted concurrency test, readiness/DR
  smoke, advisory scan, and normal `./bin/validate`.
- Grader: p99 or lock-hold evidence improves or remains bounded; worker
  freshness stays green; advisory output is clean or explicitly waived.
- Evidence packet: benchmark/advisory transcript and runtime-hardening receipt.

## Notes
This epic collects the operator overlay's `bcrypt-outside-mutex fix; tracing;
059 anyhow bump` plus existing followups `056`, `058`, and `059`. Keep the
single-writer invariant from `CLAUDE.md`; do not add hidden writer pools.

## Children
1. Verify API keys without holding the global store mutex; add a regression
   test or benchmark for authenticated ingest concurrency.
2. Execute `056-report-lock-and-owner-scope-followups.md`.
3. Set or enforce `CANARY_REQUIRE_LITESTREAM=1` for production, or add an
   explicit alert path for fail-open backup.
4. Adopt `tracing` for server/workers/operator paths.
5. Execute `059-rustsec-anyhow-bump.md`.
6. Execute `058-cadence-aware-sli-trajectory-floor.md`.
