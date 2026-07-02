# Remove the hardcoded operator domain from CLI test fixtures

Priority: P2 · Status: ready · Estimate: S

## Goal
No test fixture or default in the codebase references the operator's personal domain, so the repo is portable for external adopters.

## Oracle
- [ ] `rg "phaedrus\.io" ~/Development/canary` returns zero hits in code and fixtures (docs describing history are fine).
- [ ] The affected tests in `crates/canary-cli/src/lib.rs` pass using neutral example domains (`example.com` / env-injected values).

## Notes
Found by the 2026-07-01 adoptability audit: canary ranked #1 most stranger-adoptable (8/10) and this was its ONE named blocker. Mechanical find-and-replace plus test run — no behavior change.
**Why:** adoptability audit, rank-1 finding; canary already has an external user and a published v1.0.0, so portability debt is live product debt.
