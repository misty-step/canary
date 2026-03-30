# Monorepo Bootstrap And Contributor Path

Priority: medium
Status: ready
Estimate: M

## Goal
Give a fresh worktree one obvious bootstrap and validation path across the root app, triage app, SDKs, and clients, with root-level docs that explain ownership and setup.

## Non-Goals
- Introduce heavyweight monorepo orchestration for its own sake
- Replace language-specific tooling that already works
- Hide package-specific setup only inside issue threads or chat history

## Oracle
- [ ] Given a fresh clone, when a contributor follows the root docs, then they can bootstrap all maintained packages from one documented path
- [ ] Given local validation should mirror CI, when a contributor follows the root docs, then one documented command path covers the expected fast checks
- [ ] Given the repo is a monorepo, when the top-level docs are read, then package ownership, environment variables, and contribution expectations are described at the correct scope
- [ ] Given local feedback should happen before commit, when the chosen hook path is installed, then it runs the agreed fast validation subset before commit

## Notes
This merges the useful core of GitHub #100 and #102. It is worthwhile, but it should sit behind product-loop work instead of displacing it.
