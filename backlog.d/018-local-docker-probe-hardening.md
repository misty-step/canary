# Local Docker probe hardening

Priority: medium
Status: ready
Estimate: M

## Goal
Harden local Docker-runtime detection so repo-local validation behaves
predictably across slower machines, alternative Docker installations, and future
toolchain changes.

## Non-Goals
- Replacing Dagger as the CI control plane
- Reworking GitHub Actions topology
- Changing project quality gates or reducing strictness

## Oracle
- [ ] Given `bin/dagger` and `bin/bootstrap` both need Docker-runtime probing, when the implementation changes, then one shared helper owns timeout and fallback behavior
- [ ] Given local Docker startup can be slow, when operators need extra headroom, then the probe timeout is configurable without editing repo scripts
- [ ] Given `bin/dagger` falls back from direct Docker to Colima in `auto` mode, when fallback happens, then the operator gets an explicit note about which backend was selected
- [ ] Given the CI contract harness simulates missing Docker and Colima binaries, when tool locations or base images change, then the simulation remains hermetic and behavior tests stay stable

## Notes
Review on 2026-04-14 consistently found the current Dagger-local hardening
ready to ship, but highlighted a second-wave cleanup worth doing next:
duplicate probe logic in `bin/dagger` and `bin/bootstrap`, a fixed ~3s timeout
that can be pessimistic on slow Docker startup, silent fallback selection in
`auto` mode, and contract-shim logic that still depends on ambient command
layout.
