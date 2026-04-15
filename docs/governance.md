# Repository Governance

Canary is a solo-maintained observability service. This document records the
governance posture that cannot be encoded in repository files alone, so the
expected state is auditable at any time.

Review routing lives in [`.github/CODEOWNERS`](../.github/CODEOWNERS).
Vulnerability reporting lives in [`SECURITY.md`](../SECURITY.md). Everything
else that depends on GitHub settings is listed below.

## Expected GitHub Settings

These settings must be kept in the listed state. A drift here is a governance
incident, not a cosmetic problem. Audit quarterly and after any settings
migration.

### Branch protection — `master`

- Require a pull request before merging.
- Require review from a Code Owner (`.github/CODEOWNERS`).
- Dismiss stale approvals on new commits.
- Require status checks to pass before merging:
  - `ci / check` (Dagger pipeline in `.github/workflows/ci.yml`)
- Require branches to be up to date before merging.
- Require linear history.
- Require signed commits.
- Restrict who can push to matching branches to the repository owner.
- Do not allow force pushes.
- Do not allow deletions.

Rationale: master is the deploy branch for `canary-obs`. The CI check is the
only supported path to it.

### Secret scanning

- Secret scanning: **enabled**.
- Push protection: **enabled** (block pushes that introduce new secrets).
- Alerts routed to the repository owner.

Rationale: the API key surface (ingest keys, bootstrap key) makes accidental
leakage a P0 issue. Push protection is the cheapest backstop.

### Dependabot

- Dependabot security updates: **enabled**.
- Dependabot version updates: **enabled** for `mix`, `github-actions`, and
  `docker` ecosystems.
- Grouped updates where supported to avoid PR noise.

Rationale: Elixir/OTP, Phoenix, and the Dagger action surface are all
attack-relevant dependency axes.

### Code scanning

- GitHub-native code scanning (CodeQL or equivalent): **enabled** for the
  primary languages in use (`ruby`/`elixir` via third-party actions where
  CodeQL coverage is incomplete).

### Access

- Default repository role: **Read**.
- Collaborators: repository owner only until a second maintainer is added.
- Deploy keys: disabled unless documented here with rotation cadence.

## Verifying the posture

From a shell with `gh` authenticated against the repo owner:

```bash
gh api repos/misty-step/canary/branches/master/protection
gh api repos/misty-step/canary/vulnerability-alerts
gh api repos/misty-step/canary/code-scanning/default-setup
gh api repos/misty-step/canary/automated-security-fixes
```

A missing or `404` response for any of the above is a drift finding.

## Secret material in the working tree

Local development occasionally produces secret material (self-signed certs,
service-account JSON, `.env.*` overlays). The project `.gitignore` ignores
the common patterns; operators are still responsible for keeping
secrets outside the working tree whenever possible. See
[`.gitignore`](../.gitignore) for the current coverage.

If a secret is ever committed, rotate it before force-removing from history.
Rotation procedure for Canary-issued keys is covered in
[`SECURITY.md`](../SECURITY.md).
