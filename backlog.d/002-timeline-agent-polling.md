# Enrich timeline for agent polling and replay

Priority: high
Status: ready
Estimate: S

## Goal
Make the timeline API the canonical replay mechanism for always-on agent consumers, so webhooks are notifications and timeline is the source of truth.

## Non-Goals
- Replace webhooks — they remain the push-notification layer
- Build a full event sourcing system
- Add websocket/SSE streaming (polling with cursors is sufficient)

## Oracle
- [ ] Given a timeline query, when `GET /api/v1/timeline?event_type=incident.opened,error.new` is called, then only matching event types are returned (not all timeline events)
- [ ] Given a timeline entry for an incident or error event, when the entry is inspected, then it contains the same context as the corresponding webhook payload (service, severity, summary, group_hash, incident_id) — no follow-up query needed
- [ ] Given an agent that missed webhooks during downtime, when it polls `GET /api/v1/timeline?after=<last_cursor>&event_type=incident.opened`, then it receives all events since its last checkpoint in order
- [ ] Given the webhook-as-notification + timeline-as-replay pattern, when the API docs or project.md are inspected, then the pattern is documented as the canonical agent integration model
- [ ] Given `mix test` runs, then event-type filtering, payload enrichment, and cursor-based replay are covered

## Notes
The timeline already supports cursor-based pagination. This item adds two things:
1. Event-type filtering (so agents don't page through health check probes to find incidents)
2. Payload enrichment (so timeline entries carry enough context to act on without follow-up queries)

The integration pattern for always-on agents becomes:
- Webhook fires → agent wakes up → polls timeline from last cursor → processes new events → annotates (via 001-annotations-api)
- On crash/restart → agent polls timeline from last persisted cursor → catches up → resumes

This makes agent integration crash-safe without delivery acknowledgment endpoints or at-least-once guarantees in the webhook layer.

Parallel with: 001-annotations-api.md (no dependency between them).
Feeds into: 010-ramp-pattern.md (agent polling loop).
