# CI Control Plane

Canary's required GitHub CI runs through an immutable control plane so a pull
request cannot weaken required checks by editing `.github/workflows/ci.yml` or
the Dagger module in the candidate diff.

## Model

- Pull request enforcement uses the `pull_request_target` event, so the
  workflow definition comes from the base branch context rather than the
  candidate diff.
- The workflow checks out two trees:
  - `.ci/trusted`: the base-branch snapshot at `github.event.pull_request.base.sha`
    or `github.sha` on `push`
  - `.ci/candidate`: the candidate snapshot at
    `github.event.pull_request.head.sha` or `github.sha` on `push`
- Both checkouts set `persist-credentials: false` so the candidate tree does
  not inherit a writable Git credential helper.
- The Dagger engine version is read from `.ci/trusted/dagger.json`.
- Pull-request CI runs from the trusted checkout with both trees declared:

```bash
dagger call strict --source=../candidate --base=../trusted
```

This preserves the canonical `strict` Dagger entrypoint while making the
workflow and active module definition branch-independent for pull requests.
The trusted module hashes both trees and permits a reduced gate only when every
changed path is documentation (`docs/**` or a declared root documentation
file). Empty, mixed, deleted, and unknown-path diffs fail closed to the full
gate. Candidate code never chooses its own scope or lowers the trusted Rust
coverage floor.

Without `--base`, including local `./bin/validate --strict` and non-PR hosted
runs, scope classification fails closed to the full runtime gate.

The trusted validator structurally pins `strict` authority bindings and exact
invocations, then hashes normalized bodies for the small set of functions that
own policy mounting, scope classification, Rust gates, and production-image
rehearsal. Comments and dead-code decoys therefore cannot preserve a weakened
contract. A deliberate control-plane rewrite uses two PRs: first expand the
trusted digest allowlist while the old body remains valid, then change the body
and retire the old digest after the new policy is on the base branch.

## Full runtime profile

The full strict gate includes two regression oracles beyond ordinary package
tests:

- Rust workspace line coverage must remain at or above 90%. The trusted module
  reads both trees' numeric `dagger/policy/rust-coverage-floor` declarations
  and refuses a candidate below the trusted declaration or compiled floor
  before invoking pinned `cargo-llvm-cov` 0.8.7. A merged floor increase thus
  becomes the next PR's minimum. The plain numeric declaration avoids
  source-parser or comment-spoofing ambiguity.
- The production image is started over a migrated SQLite database containing
  50,000 current errors across 200 services plus 20,000 expired errors. With 16
  concurrent clients, the gate interleaves 20 query and 20 report samples with
  ingest and readiness requests. Query and report use separate read identities
  so the workload stays within Canary's per-key product quota while preserving
  statistically useful p95 samples. The gate waits for full readiness, then
  requires zero HTTP errors and zero 5xx responses; requires the retention
  worker to delete all 20,000 expired rows; and enforces p95 ceilings of 2,000
  ms for query and 4,000 ms for report.

The load rehearsal uses a dedicated service instance. Existing integration
smokes retain their small fixture so one oracle cannot distort another.

## Update Procedure

1. Edit the Dagger module and, if needed, `.github/workflows/ci.yml` in a
   normal pull request.
2. Run `python3 dagger/scripts/ci_contract_validation.py`.
3. Run `./bin/validate --strict`.
4. Merge the pull request to `master`.
5. Subsequent pull requests will use the new control plane because
   `pull_request_target` evaluates from the updated base branch.

## Pins

- GitHub checkout action is pinned by commit in `.github/workflows/ci.yml`.
- `dagger/dagger-for-github` is pinned by commit in `.github/workflows/ci.yml`.
- The Dagger engine version is pinned in `dagger.json` and consumed from the
  trusted checkout.
