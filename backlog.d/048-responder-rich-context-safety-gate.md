# Gate rich responder context behind least privilege and audit

Priority: P0 · Status: pending · Estimate: XL

## Goal
Make Canary safe for arbitrary-user auto-triage by ensuring external responders receive only authorized, minimized, replayable, and auditable context.

## Oracle
- [ ] Given a responder needs to claim or annotate one service's incident, then Canary can issue a narrow responder-write authority that permits claims and annotations for that service without broad admin access.
- [ ] Given an incident, error group, target, or monitor is serialized for a responder, then the payload is produced through a redacted context schema that includes tenant, project, service, subject, retention, and privacy policy.
- [ ] Given telemetry attributes, annotation metadata, and evidence links contain sensitive-looking data, then responder context either redacts/minimizes it server-side or excludes it by schema.
- [ ] Given a webhook receiver is registered as an automation responder, then a conformance fixture proves timestamp validation, delivery-id dedupe, and timeline replay before action.
- [ ] Given a responder reads rich incident detail, then Canary records a durable read-audit event with responder identity, subject, context envelope, and timestamp.

## Notes
Why: the security lane found tenant/project/service isolation is mostly in place, but rich-context producers must never omit authority. Claims currently require admin, service-bound admin keys are rejected for mutation, telemetry privacy labels are policy-only, and there is no per-responder read receipt. That is acceptable for owned dogfood; it is not acceptable for arbitrary-user responders.

This ticket should land before promoting Canary as a general hosted substrate for third-party automated remediation.

## Children
1. Design `responder-write` or equivalent least-privilege scope.
2. Define responder context schemas per subject/event type.
3. Add server-side minimization/redaction for rich context or explicitly exclude risky fields.
4. Add webhook receiver conformance fixtures and docs.
5. Add read-audit timeline events for rich context fetches.
6. Update OpenAPI agent guidance with the safety contract.
