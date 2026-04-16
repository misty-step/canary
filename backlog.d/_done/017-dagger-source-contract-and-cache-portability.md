# Dagger source contract and cache portability

Priority: medium
Status: done
Estimate: M

## Goal
Eliminate drift-prone Dagger source-filter configuration and make cache identity
explicitly portable across execution platforms.

## Non-Goals
- Rewriting the CI pipeline away from Dagger
- Broad perf tuning outside Dagger cache/source concerns
- Solving unrelated repo hygiene issues

## Oracle
- [x] Given the repo source exclusion list changes, when Dagger entrypoints are updated, then one authoritative definition feeds every exposed function
- [x] Given `./bin/validate --fast` and `./bin/validate --strict` run, when their Dagger entrypoints consume repo source, then they see the same source-filter contract
- [x] Given validation runs on different architectures or runner environments, when caches are reused, then incompatible build artifacts are not shared across platform boundaries
- [x] Given Dagger's TypeScript introspector still constrains `@argument` metadata, when the final solution lands, then the repo documents or generates the pattern so future edits cannot silently drift

## Notes
Review feedback on 2026-04-08 flagged two related issues in `dagger/src/index.ts`:
the duplicated inline `ignore` list required by the current Dagger TypeScript
introspector behavior, and cache keys that still do not encode platform
identity. This likely needs either an upstream-compatible generation pattern or
a documented local workaround that reduces drift without breaking `defaultPath`.

## What Was Built
- Added `dagger/scripts/sync_source_arguments.py` as the authoritative source
  of truth for every public Dagger `Directory` argument ignore list, with a
  `--write` sync mode that regenerates the required inline literals in
  `dagger/src/index.ts`.
- Wired `dagger/scripts/ci_contract_validation.py` to enforce the sync
  contract and to fail if cache keys stop including the Dagger default
  platform, image identity, and lockfile digest.
- Scoped the Elixir and Node dependency cache volume names in
  `dagger/src/index.ts` by `dag.defaultPlatform()` so local arm64 and remote
  amd64 validation runs no longer share incompatible cache volumes.
- Documented the source-filter sync workflow in `README.md` so future edits
  update the policy table instead of hand-editing duplicated literals.

## Verification
- `python3 dagger/scripts/ci_contract_validation.py`
- `./bin/validate --fast`
- `./bin/validate --strict`
