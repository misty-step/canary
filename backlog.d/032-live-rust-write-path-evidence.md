# Prove Rust production write paths with evidence packets

Priority: high
Status: ready
Estimate: L

## Goal
Prove the deployed Rust service handles Canary's admin, ingest, health, webhook, and worker-backed write paths in production-like conditions, with sanitized evidence packets agents can replay.

## Oracle
- [ ] A new evidence packet under `docs/architecture/` records the exact current commit, Fly image, machine version, commands, redacted responses, and cleanup proof for the write-path run.
- [ ] The run exercises `POST /api/v1/errors` through query/report/timeline readback, target create/pause/resume/delete, monitor create/check-in/delete, webhook create/test/delivery lookup/delete, API key mint/revoke, and DR status.
- [ ] Each created production or staging resource is uniquely named, queried back, and cleaned up; the packet includes the post-cleanup query proving no disposable resource remains.
- [ ] README and Rust cutover docs distinguish read-path cutover proof from write-path proof and do not overclaim unverified production behavior.
- [ ] `./bin/validate --fast` is green after any docs or harness updates, and no API key or secret value appears in the packet or git diff.

## Notes
**Why:** Product/operator perspective. The 2026-06-06 Rust cutover packet proves deploy, `/healthz`, `/readyz`, DR status, and authenticated read routes, but explicitly leaves admin, ingest, webhook, retention, target-probe, TLS-scan, and monitor paths unproven under production load.

**Children**
1. Add a reusable write-path rehearsal script or runbook that names every disposable resource and captures redacted JSON.
2. Run the rehearsal against `canary-obs` or a production-shaped Fly clone and publish the evidence packet.
3. Update docs so future Rust production claims point at the specific packet by path and date.

**Responder-boundary check.** Canary proves observability write/read surfaces. Downstream triage, repo mutation, and fix proposals stay outside this repo.
