# Immutable CI control plane

Priority: medium
Status: ready
Estimate: M

## Goal
Move GitHub-required CI enforcement onto a control plane that a pull request
cannot weaken by editing in-repo workflow or Dagger module files.

## Non-Goals
- Replacing GitHub entirely as remote hosting in this pass
- Removing the local-first `./bin/validate --strict` workflow
- Rebuilding CI around a second orchestration system

## Oracle
- [ ] Given a pull request changes `.github/workflows/ci.yml` or `dagger/src/index.ts`, when required GitHub checks run, then the definition of those required checks still comes from a trusted, immutable source outside the candidate diff
- [ ] Given CI policy changes are needed, when the operator updates the control plane, then the change happens in one dedicated location with explicit version pinning and reviewable rollout steps
- [ ] Given a branch attempts to remove or weaken a required Dagger phase, when GitHub CI runs, then the branch cannot suppress that phase by editing repo-local wrapper files alone
- [ ] Given local validation remains Dagger-first, when `./bin/validate --strict` runs, then the local workflow still maps cleanly onto the remote required phases

## Notes
Review feedback on 2026-04-08 flagged that GitHub CI still evaluates a mutable,
in-repo Dagger module and workflow definition. That is acceptable for now but
not sufficient for a hardened control plane. Likely solutions include a pinned
reusable workflow in a trusted repo, a pinned external Dagger module, or a
branch-independent verification layer owned outside this repository.
