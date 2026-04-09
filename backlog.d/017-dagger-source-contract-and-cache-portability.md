# Dagger source contract and cache portability

Priority: medium
Status: ready
Estimate: M

## Goal
Eliminate drift-prone Dagger source-filter configuration and make cache identity
explicitly portable across execution platforms.

## Non-Goals
- Rewriting the CI pipeline away from Dagger
- Broad perf tuning outside Dagger cache/source concerns
- Solving unrelated repo hygiene issues

## Oracle
- [ ] Given the repo source exclusion list changes, when Dagger entrypoints are updated, then one authoritative definition feeds every exposed function
- [ ] Given `./bin/validate --fast` and `./bin/validate --strict` run, when their Dagger entrypoints consume repo source, then they see the same source-filter contract
- [ ] Given validation runs on different architectures or runner environments, when caches are reused, then incompatible build artifacts are not shared across platform boundaries
- [ ] Given Dagger's TypeScript introspector still constrains `@argument` metadata, when the final solution lands, then the repo documents or generates the pattern so future edits cannot silently drift

## Notes
Review feedback on 2026-04-08 flagged two related issues in `dagger/src/index.ts`:
the duplicated inline `ignore` list required by the current Dagger TypeScript
introspector behavior, and cache keys that still do not encode platform
identity. This likely needs either an upstream-compatible generation pattern or
a documented local workaround that reduces drift without breaking `defaultPath`.
