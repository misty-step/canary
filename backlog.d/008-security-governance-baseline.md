# Security and governance baseline

Priority: medium
Status: ready
Estimate: S

## Goal
Make review routing and security reporting explicit. SECURITY.md exists; CODEOWNERS does not.

## Non-Goals
- Claim a GitHub setting is enabled without verification
- Replace enforcement with prose when a repository file can encode the policy
- Relax any existing quality gate

## Oracle
- [ ] Given the baseline is complete, when the repo is inspected, then `CODEOWNERS` exists and matches current ownership expectations
- [ ] Given the repo contains local secret material during development, when `.gitignore` is inspected, then common key and certificate patterns are ignored
- [ ] Given some GitHub settings cannot be enforced from code alone, when repo docs are read, then the expected state for branch protection, secret scanning, and Dependabot is stated

## Notes
SECURITY.md already shipped. Remaining: CODEOWNERS + documented GitHub settings expectations.
Migrated from .backlog.d/007.
