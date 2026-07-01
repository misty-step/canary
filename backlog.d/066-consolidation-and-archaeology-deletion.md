# Delete parity archaeology and shallow runtime duplication

Priority: P2 · Status: pending · Estimate: XL

## Goal
Reduce Canary's maintenance surface by deleting or consolidating Elixir-era
parity artifacts, duplicated worker lifecycle machinery, stale backlog roots,
and shallow seams that no longer earn their complexity.

## Oracle
- [ ] Given the five background workers, then lifecycle management is shared by
      one deep module or clearly justified where it remains bespoke.
- [ ] Given webhook jobs no longer need Oban compatibility, then job storage is
      named for Canary's current Rust model or the compatibility reason is
      documented.
- [ ] Given `canary-ingest` remains a crate, then its boundary is deep enough
      to justify the dependency; otherwise validation vocabulary moves to the
      owning crate.
- [ ] Given legacy fixture DBs and Phoenix/Oban comments are scanned, then only
      migration-critical coverage remains.
- [ ] Given Dagger package managers are scanned, then only the intended lockfile
      remains.
- [ ] Given `.backlog.d/` and `.codex/agents/` are tracked, then they are
      archived, deleted, or justified by a current harness consumer.
- [ ] Given `AGENTS.md` names known debt, then it points to the current
      factory-groom epics instead of stale 010/020-only debt.

## Verification System
- Claim: Canary can shed thousands of lines of shallow or stale surface without
  weakening behavior.
- Falsifier: consolidation obscures worker failures, deletes migration evidence
  still needed for production DBs, or removes harness artifacts another active
  tool still consumes.
- Driver: focused refactor PRs with existing worker/readiness tests, migration
  fixture review, grep receipts, and `./bin/validate`.
- Grader: behavior stays identical, fewer code paths own each concept, and
  deleted artifacts have explicit replacement or archive rationale.
- Evidence packet: deletion receipt with line-count delta and surviving
  compatibility reasons.

## Notes
This is the groom report's deletion/consolidation epic. It is intentionally
below product and runtime safety work. Prefer small focused PRs; do not mix
schema-affecting job migration with worker lifecycle refactor.

## Children
1. Replace duplicated worker lifecycle quads with one deep lifecycle module.
2. Decide and execute the Oban-compatible job storage rename/migration or write
   the compatibility rationale.
3. Fold or justify `canary-ingest`.
4. Retire legacy DB fixtures down to the minimum migration oracle.
5. Delete the unused `Burst` rate-limit bucket after confirming no contract
   depends on it.
6. Keep one Dagger package-manager lockfile.
7. Merge/archive `.backlog.d/` into `backlog.d/_done/` and remove the hidden
   root.
8. Decide `.codex/agents/` ownership and delete or document it.
9. Refresh `AGENTS.md` known-debt map after the new epics land.
