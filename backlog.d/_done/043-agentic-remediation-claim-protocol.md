# Add a typed agentic remediation claim protocol

Priority: P1
Status: done
Estimate: L

## Goal
Let multiple agents coordinate incident/error/target/monitor follow-up without duplicate remediation by adding typed claim, state, idempotency, and completion semantics on top of Canary's durable evidence plane.

## Oracle
- [ ] Agents can atomically claim a durable subject (`incident`, `error_group`, `target`, or `monitor`) with owner, purpose, TTL, idempotency key, and evidence links.
- [ ] Conflicting claims return deterministic Problem Details responses and expose the current owner/state without requiring clients to parse free-form annotations.
- [ ] Agents can transition claims through bounded states such as `claimed`, `investigating`, `fix_proposed`, `verified`, `dismissed`, `expired`, and `released`.
- [ ] Timeline, incident detail, query/report, and annotations expose claim state in bounded payloads.
- [ ] Webhooks emit claim lifecycle hints while the timeline remains the durable source of truth.
- [ ] CLI/MCP helpers support claim/read/transition/release flows for responder agents.

## Children
1. Decide whether claims are a strict annotation subtype or a new table with annotation mirrors.
2. Add atomic claim create/read/transition/release routes and storage.
3. Add timeline/webhook events and incident/query/read-model surfacing.
4. Add CLI/MCP helpers and OpenAPI agent guidance.
5. Update responder docs to use typed claims instead of opaque annotation conventions.

## Notes
- Evidence: `priv/openapi/openapi.json` and `crates/canary-server/src/annotations.rs` already support annotations on incidents, error groups, targets, and monitors, but action/metadata are opaque and do not enforce ownership.
- Agent-readiness lane found this is the concurrency gap for automated triage and remediation handoff.
- This does not move repo mutation into Canary. Canary owns coordination state; downstream systems own repo changes.
