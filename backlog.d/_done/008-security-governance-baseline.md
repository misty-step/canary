# Security and governance baseline

Priority: medium
Status: done
Estimate: S

## Goal
Make review routing and security reporting explicit. SECURITY.md exists; CODEOWNERS does not.

## Non-Goals
- Claim a GitHub setting is enabled without verification
- Replace enforcement with prose when a repository file can encode the policy
- Relax any existing quality gate

## Oracle
- [x] Given the baseline is complete, when the repo is inspected, then `CODEOWNERS` exists and matches current ownership expectations
- [x] Given the repo contains local secret material during development, when `.gitignore` is inspected, then common key and certificate patterns are ignored
- [x] Given some GitHub settings cannot be enforced from code alone, when repo docs are read, then the expected state for branch protection, secret scanning, and Dependabot is stated

## Notes
SECURITY.md already shipped. Remaining: CODEOWNERS + documented GitHub settings expectations.
Migrated from .backlog.d/007.

## What Was Built

- Added `.github/CODEOWNERS` routing all reviews to `@phrazzld` with directory-scoped
  rules for security surfaces (SECURITY.md, `.github/`, governance docs) and the CI
  control plane (workflows, `dagger.json`, `dagger/`, `fly.toml`, `Dockerfile`).
- Expanded `.gitignore` with patterns for common secret material —
  `*.pem`/`*.key`/`*.crt`/`*.p12`/`*.pfx`/`*.jks`, SSH private keys
  (`id_rsa*`, `id_ed25519*`, `id_ecdsa*`, `*_rsa`/`*_ed25519`/`*_ecdsa`),
  credential JSON, `.env.*` overlays (with `!.env.example` allowlist), and
  `*.tfvars`. Prevents the common accidental-commit vectors.
- Added `docs/governance.md` recording expected GitHub settings that cannot be
  encoded in repo files: branch protection on `master` (CODEOWNERS review, Dagger
  CI check required, linear history, signed commits, no force pushes, no
  deletions), secret scanning with push protection, Dependabot security + version
  updates for `mix`/`github-actions`/`docker`, code scanning, and access posture.
  Includes `gh api` verification commands for quarterly drift audits.
- Linked the governance doc from `SECURITY.md` so reporters and reviewers can
  discover the full policy surface from one entry point.

## Verification

- `mix format --check-formatted` — clean
- `git check-ignore -v .github/CODEOWNERS docs/governance.md` — both tracked, not ignored
- Manual inspection of repo for oracle criteria — all three satisfied
