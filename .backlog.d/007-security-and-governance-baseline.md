# Security And Governance Baseline

Priority: medium
Status: ready
Estimate: S

## Goal
Make review routing and security reporting explicit, and document the exact GitHub repo settings Canary expects for branch protection and secret hygiene.

## Non-Goals
- Claim a GitHub setting is enabled without verification
- Replace enforcement with prose when a repository file can encode part of the policy
- Relax any existing quality gate

## Oracle
- [ ] Given the baseline is complete, when the repo is inspected, then `CODEOWNERS` and `SECURITY.md` exist and match current ownership expectations
- [ ] Given the repo contains local secret material during development, when `.gitignore` is inspected, then common key and certificate patterns are ignored where appropriate
- [ ] Given some GitHub settings cannot be enforced from code alone, when the repo docs are read, then the expected state for branch protection, secret scanning, and Dependabot security updates is stated exactly

## Notes
This is GitHub #101, kept as a lower-priority repo-hardening item. It matters, but it is not the next product move.
