# Local Docker probe hardening

Priority: medium
Status: done
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

## What Was Built

- Consolidated local Docker-runtime probing into `bin/lib/docker_probe.sh`, so `bin/dagger` and `bin/bootstrap` now share one implementation for timeout handling, Colima readiness checks, backend selection, and operator-facing error text.
- Added configurable probe headroom through `CANARY_DOCKER_PROBE_TIMEOUT_SECONDS` while preserving the existing tick-based override, so slow local Docker startups can be tolerated without editing repo scripts.
- Made `bin/dagger` `auto` mode announce when it falls back from direct Docker access to Colima over SSH, with distinct notes for unavailable, timed-out, and failed direct probes.
- Hardened the CI contract harness to simulate missing Docker, timed-out probes, and Colima fallback hermetically, including an explicit timeout-seconds regression check.

## Verification

- `bash -n bin/lib/docker_probe.sh && bash -n bin/dagger && bash -n bin/bootstrap`
- `python3 -m py_compile dagger/scripts/ci_contract_validation.py`
- `python3 dagger/scripts/ci_contract_validation.py`
- `./bin/validate --strict`
