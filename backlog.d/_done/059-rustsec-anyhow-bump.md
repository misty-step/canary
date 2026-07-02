# Bump anyhow after RUSTSEC-2026-0190

Priority: P2 · Status: pending · Estimate: S

## Goal
Curate the dependency bump that clears `RUSTSEC-2026-0190` for `anyhow`
without turning the advisory fix into an unreviewed lockfile churn pile.

## Why now
The 2026-07-01 strict gate reports `RUSTSEC-2026-0190` as an allowed warning
for `anyhow 1.0.102`: unsoundness in `Error::downcast_mut()`. It is currently
allowed by the advisory lane, but the warning should not become permanent
background noise.

## Oracle
- [ ] `cargo update -p anyhow` or the smallest equivalent curated update clears
      `RUSTSEC-2026-0190`.
- [ ] `./bin/validate --advisories` no longer reports the warning.
- [ ] `./bin/validate` passes after the lockfile change.
- [ ] The PR body names the advisory id and the exact dependency delta.

## Verification System
- Claim: the advisory is resolved by a narrow dependency update.
- Falsifier: the update leaves the advisory present, pulls unrelated major
  dependency churn, or requires lowering gates.
- Driver: advisory scan before/after plus the normal Canary validation gate.
- Grader: advisory absent, lockfile diff reviewed, and strict gate green.
- Evidence packet: `cargo update` transcript, advisory scan output, and
  `./bin/validate` transcript.

## Notes
This is deliberately separate from feature work. Dependency upgrades ride as
one curated, risk-assessed commit.
