# Harden the triage contract against duplicate or runaway responders

Priority: P1 · Status: pending · Estimate: XL

## Goal
Make Canary's incident wake-up contract safe enough for repeated real
responders by adding durable cooldown, dispatch budget caps, and claim-gated
delivery semantics before the Factory relies on automated triage at 3am.

## Oracle
- [ ] Given repeated transitions for the same subject, then Canary enforces a
      durable cooldown that survives process restart.
- [ ] Given a responder subscription has a dispatch budget, then Canary refuses
      or suppresses deliveries after the configured cap and records why.
- [ ] Given a subject already has an active remediation claim, then configured
      claim-gated delivery sends subsequent work only to the claim holder or
      returns a claim-held hint.
- [ ] Given a claim holder crashes, then TTL expiry and reclaim events prevent
      silent incident starvation.
- [ ] Given a responder fixture receives deliveries, then signature validation,
      delivery-id dedupe, timeline replay, claim-before-work, and annotation
      writeback are conformance-scored.
- [ ] Given drill traffic is enabled, then synthetic drill events are scoped so
      they do not pollute production SLI or normal incident history.

## Verification System
- Claim: Canary can wake automated responders without duplicate work storms or
  unbounded spend.
- Falsifier: process restart forgets cooldown, one flap emits uncapped
  duplicate deliveries, non-claim holders continue receiving work, or a crashed
  claim holder silences a subject indefinitely.
- Driver: webhook delivery tests, claim lifecycle tests, restart/cooldown
  fixtures, and an end-to-end `canary drill` receipt.
- Grader: delivery ledger explains suppressed, capped, claimed, and delivered
  outcomes deterministically.
- Evidence packet: conformance fixture output and drill receipt under
  `docs/architecture/`.

## Notes
This epic folds in the groom report's creative pass: claim-gated delivery and a
continuous fire-drill. Canary still does not mutate repos or call models; it
enforces coordination and budget boundaries while Bitterblossom owns the
responder workload economics.

## Children
1. Persist cooldown state instead of relying on in-process TTL maps.
2. Add responder dispatch budget configuration and ledgered suppression.
3. Add claim-gated delivery semantics with claim-held hints.
4. Prove claim TTL/reclaim behavior for crashed responders.
5. Add webhook responder conformance fixtures.
6. Extend `bin/canary-write-path-rehearsal` or add `canary drill` for scored
   synthetic triage runs.
7. Keep drill accounting separate from normal SLI and incident views.
