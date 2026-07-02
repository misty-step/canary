# Webhook receiver conformance fixtures (signature, timestamp, dedupe, replay)

Priority: P1 · Status: ready · Estimate: M

## Goal
A receiver-side conformance test suite proves Canary's existing signed-webhook
contract (already implemented, not yet fixture-tested end to end) actually
does what the docs and the responder contract claim: valid signatures pass,
tampered/expired/replayed deliveries are rejected, and duplicate delivery ids
are deduped.

## Oracle
- [ ] Fixtures cover, against the real functions in
      `crates/canary-http/src/webhooks.rs` (`verify_signature`,
      `sign_timestamped`/`timestamped_signature_header`, delivery envelope
      `timestamp.delivery_id.body`):
      - valid signature + fresh timestamp + unseen delivery id → accepted
      - tampered body with an otherwise-valid signature → rejected
      - stale/out-of-window timestamp → rejected
      - replayed delivery id (same id delivered twice) → second delivery is a
        no-op, not double-processed
      - malformed/missing signature header → rejected
- [ ] Fixtures live as committed test data (not only inline test code) so a
      future responder implementation (e.g. Bitterblossom's triage workload)
      can run the same fixtures against its own receiver and prove conformance
      without depending on Canary's Rust test harness.
- [ ] Tests fail loudly (not silently skip) if `verify_signature` or the
      timestamp/dedupe window ever changes shape.
- [ ] `./bin/validate` passes.

## Notes
The groom sweep found real signature/timestamp/dedupe code
(`canary-http/src/webhooks.rs`) but no dedicated conformance test surface —
`find . -iname "*conformance*"` and `*webhook*test*` come up empty outside
inline unit tests. This is pure testing-of-existing-behavior: no new auth
model, no new scopes, no design call. It directly satisfies the P1 line in
`backlog.d/048-responder-rich-context-safety-gate.md` ("ship receiver
conformance fixtures for signature timestamp validation, delivery-id dedupe,
and timeline replay before action") and feeds the conformance-scoring bullet
in `backlog.d/063-triage-contract-hardening.md` without touching either
ticket's larger cooldown/budget-cap/claim-gating scope.

**Why:** the triage loop is the product's whole reason for existing and today
has "one $0.018 synthetic run" as its only end-to-end proof (per the 2026-07-01
groom report). Fixture-level conformance tests are the cheapest available way
to raise confidence in the receiver contract before 063's harder enforcement
work lands.
