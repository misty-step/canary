# Gate external responders behind least privilege and audited context

Priority: P0 · Status: pending · Estimate: XL

## PRD Summary
- User: external remediation agents that wake from Canary signals and need safe context before acting.
- Problem: arbitrary responders currently need broad authority for claim/evidence writes, and rich context reads do not yet have enforced minimization or read-audit semantics.
- Goal: make external responder context authorized, minimized, replayable, auditable, and safe for arbitrary-agent consumers.
- Why now: Canary has claims, annotations, incidents, telemetry, and webhooks; before broader integrations or public/browser capture, the responder trust boundary must be explicit.
- UX enabled: a responder can claim and annotate one service's subject with narrow authority, read a minimized context envelope, and leave an audit trail without admin-level access.
- Deliverable type: working code, contract docs, conformance fixtures, and safety tests.
- Success signal: a service-bound responder can complete claim/read/annotate/replay conformance while over-scoped, cross-service, unredacted, or unaudited access fails.

## Product Requirements
- P0: provide a narrow responder-write authority for claims and annotations without broad admin access.
- P0: serialize responder context through subject-specific envelopes with service, tenant/project, retention, and privacy policy metadata.
- P0: enforce server-side minimization/redaction for telemetry attributes, annotation metadata, evidence links, and responder context.
- P0: add durable read-audit events for rich responder context fetches.
- P0: define safe browser/public-ingest authority or relay semantics before any client-side capture path exposes credentials.
- P1: ship receiver conformance fixtures for signature timestamp validation, delivery-id dedupe, and timeline replay before action.
- Non-goals: repo mutation, issue creation, autonomous deploy rollback, or customer incident command.

## Technical Design
- Chosen architecture: add responder-specific scopes/context envelopes as adapters over existing claims, annotations, timeline, and incident read models; keep Canary as coordinator, not fixer.
- Files/systems touched: auth scope model, claims routes, annotation routes, incident detail/read models, telemetry/context redaction, OpenAPI, CLI/MCP manifest, and webhook receiver docs/fixtures.
- Data/control flow: webhook wakes responder; responder verifies/dedupes delivery, creates or observes claim, reads minimized context, acts outside Canary, writes annotation/claim evidence, and Canary records read/write audit events.
- Build/check boundary: route tests prove least privilege and cross-service denial; contract tests prove OpenAPI scopes; fixture tests prove webhook receiver conformance.
- ADR decision: required if the scope model adds new key classes beyond existing `ingest-only`, `read-only`, and `admin`.
- ADR-style invariants: service-bound authority never escalates to all services; privacy labels are enforced before rich context leaves Canary; read-audit events do not leak redacted payloads.
- Design X vs Y: prefer responder-specific scopes and context envelopes over handing agents admin keys plus informal docs; informal docs are not a safety boundary.

## Goal
Make Canary safe for arbitrary-user auto-triage by ensuring external responders receive only authorized, minimized, replayable, and auditable context.

## Oracle
- [ ] Given a responder needs to claim or annotate one service's incident, then Canary can issue a narrow responder-write authority that permits claims and annotations for that service without broad admin access.
- [ ] Given an incident, error group, target, or monitor is serialized for a responder, then the payload is produced through a redacted context schema that includes tenant, project, service, subject, retention, and privacy policy.
- [ ] Given telemetry attributes, annotation metadata, and evidence links contain sensitive-looking data, then responder context either redacts/minimizes it server-side or excludes it by schema.
- [ ] Given a webhook receiver is registered as an automation responder, then a conformance fixture proves timestamp validation, delivery-id dedupe, and timeline replay before action.
- [ ] Given a responder reads rich incident detail, then Canary records a durable read-audit event with responder identity, subject, context envelope, and timestamp.
- [ ] Given browser capture is enabled for a consuming app, then it uses a public-ingest token or relay design that cannot read, administer, claim, or expose a secret API key.
- [ ] Given MCP or CLI tools expose responder actions, then their manifest scopes and runtime enforcement match the HTTP authority model.

## Verification System
- Claim: arbitrary external responders can act through Canary without broad authority or unminimized context.
- Falsifier: a service-bound responder can claim or read another service, rich context returns sensitive attributes, browser capture exposes a secret key, or a rich read leaves no audit event.
- Driver: route-level auth tests, OpenAPI scope contract tests, responder context snapshot tests, webhook receiver conformance fixtures, and CLI/MCP manifest checks.
- Grader: unauthorized access returns RFC 9457 Problem Details; context snapshots match redaction rules; audit timeline contains non-sensitive read/write records.
- Evidence packet: test transcripts plus a responder conformance receipt checked into `docs/architecture/` or attached to the PR.
- Cadence: every strict gate for contract tests; conformance fixture before enabling arbitrary-user responders.

## Notes
Why: the security lane found tenant/project/service isolation is mostly in place, but rich-context producers must never omit authority. Claims currently require admin, service-bound admin keys are rejected for mutation, telemetry privacy labels are policy-only, and there is no per-responder read receipt. That is acceptable for owned dogfood; it is not acceptable for arbitrary-user responders.

This ticket should land before promoting Canary as a general hosted substrate for third-party automated remediation.

## Children
1. Design `responder-write` or equivalent least-privilege scope.
2. Define responder context schemas per subject/event type.
3. Add server-side minimization/redaction for rich context or explicitly exclude risky fields.
4. Define safe browser capture through public-ingest tokens or a relay boundary.
5. Add webhook receiver conformance fixtures and docs.
6. Add read-audit timeline events for rich context fetches.
7. Align HTTP, CLI, and MCP responder authority metadata.
8. Update OpenAPI agent guidance with the safety contract.
