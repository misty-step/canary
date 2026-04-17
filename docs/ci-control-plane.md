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
- Required CI runs from the trusted checkout with:

```bash
dagger call strict --source=../candidate
```

This preserves the canonical `strict` Dagger entrypoint while making the
workflow and active module definition branch-independent for pull requests.

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
