# Make SLI trajectory sample floors cadence-aware

Priority: P2 · Status: pending · Estimate: M

## Goal
Replace the fixed `MIN_TRAJECTORY_SAMPLES` availability floor with a
cadence-aware policy so low-frequency but complete target/monitor windows can
produce trustworthy trajectory deltas, while genuinely thin windows still
return `insufficient_samples`.

## Why now
#047 child #6 deliberately shipped a conservative fixed floor of 20 samples per
current/prior window. That is safe for common 30-60s probes, but it can null a
complete 1h trajectory for a target probed every 5 minutes (about 12 expected
samples), and it can be too permissive or too strict as monitor/check-in
cadence varies. The shape packet accepted this as a follow-up rather than
blocking the final #047 slice.

## Oracle
- [ ] Given a 60s target or monitor cadence with complete current and prior 1h
      windows, then trajectory availability deltas clear the floor as they do
      today.
- [ ] Given a lower-frequency target such as a 5m probe with complete current
      and prior 1h windows, then the floor scales to expected cadence and emits
      a real availability delta instead of `insufficient_samples`.
- [ ] Given either current or prior windows miss enough expected samples for the
      configured cadence, then availability deltas remain null and trajectory
      status is `insufficient_samples`.
- [ ] Given a service has both targets and monitors with different cadences,
      then `sample_basis` or its replacement exposes enough basis metadata for
      an agent to understand which signal cleared or failed the floor.

## Verification System
- Claim: trajectory sample sufficiency reflects expected observation cadence,
  not a one-size-fits-all count.
- Falsifier: a complete low-frequency target stays `insufficient_samples`, or a
  sparse high-frequency target emits a non-null availability delta.
- Driver: `canary-store` trajectory unit tests with explicit target intervals
  and monitor cadence/TTL fixtures; report serialization tests if the wire
  metadata changes.
- Grader: deltas and `insufficient_samples` match cadence-specific expectations
  without adding a server-side burn-rate or urgency verdict.
- Evidence packet: focused test transcript plus one report JSON fixture showing
  floor basis for low-frequency and high-frequency services.
- Cadence: store tests on every gate; revisit when new monitor cadence modes are
  added.

## Relationship to existing backlog
Follow-up to #047 child #6. This keeps the #047 evidence-vs-policy boundary:
Canary may improve whether a trajectory delta is statistically trustworthy, but
it still does not compute burn-rate severity, page/ticket routing, or responder
urgency.
