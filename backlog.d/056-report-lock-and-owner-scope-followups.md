# Lower /api/v1/report store-lock contention and dedup the SLI owner-scope clause

Priority: P2 · Status: pending · Estimate: M

## Goal
Address two non-blocking findings from the #047 thermo-nuclear review (merged,
deployed) that are real store-layer debt on a hot path:

1. **Report store-lock contention.** The unified `/api/v1/report` path holds the
   single `Mutex<Store>` writer lock across ~15 sequential SELECTs (≈9
   pre-existing report sections + 6 new service-SLI aggregates). Under load this
   lengthens head-of-line blocking against the writer — ingest, target probes,
   and webhook delivery all share that lock — raising p99 ingest latency when
   reporting is hot.
2. **Duplicated owner-scope clause.** The tenant/project scoping SQL is
   duplicated between `crates/canary-store/src/query.rs` and
   `crates/canary-store/src/service_sli.rs`. It is security-relevant
   (cross-tenant isolation); two copies invite drift where one is hardened and
   the other silently is not.

## Why now
Both surfaced in the pre-merge review of #047. Neither blocked ship — the
queries are window-bounded and index-backed, and scoping is correct in both
copies today — but they are genuine perf/maintainability debt on a hot,
security-sensitive path and should not be lost to a closed PR.

## Scope / decision to record
- **Report lock:** measure first. If `/api/v1/report` p99 regresses under
  concurrent ingest, fold the SLI aggregates into fewer statements or compute
  them outside the store lock from already-fetched section data. Do **not** add
  a second writer or a hidden read pool (single-writer invariant).
- **owner_clause:** hoist the scoping clause into one `pub(crate)` helper (e.g. a
  shared `scope` module) called from both `query.rs` and `service_sli.rs`;
  reconcile the unscoped-default signature difference noted in review.

## Oracle
- [ ] A telemetry/bench check shows `/api/v1/report` holds the store lock no
      longer than (ideally measurably less than) the pre-SLI baseline under N
      concurrent reporters + ingesters.
- [ ] `grep` shows exactly one owner-scope clause builder in `canary-store`,
      used by both the query read models and the service-SLI projections.
- [ ] Tenant/project isolation behavior is unchanged — existing scoping tests
      pass; add one if a shared helper is introduced.

## Relationship to existing backlog
Follow-up to #047 (alert-plane reliability; children 1–5 shipped, child #6
burn-rate still open). Pure store-layer refactor + perf; no API/contract change.
