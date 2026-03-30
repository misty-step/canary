# Annotations API for agent-consumable incident/error metadata

Priority: high
Status: ready
Estimate: M

## Goal
Let any Canary consumer attach structured annotations to incidents and error groups — enabling triage tracking, acknowledgment, and coordination without imposing a workflow.

## Non-Goals
- Define what "triaged" means — consumers own their own annotation vocabulary
- Build claim/lease semantics or exclusive locks
- Add triage state machine to incidents — annotations are facts, not state transitions
- Dashboard UI for managing annotations (read-only display is fine)

## Oracle
- [ ] Given an authenticated API client, when `PATCH /api/v1/incidents/:id/annotations` is called with `{"agent": "bb-triage", "action": "acknowledged", "metadata": {"issue": "#42"}}`, then the annotation is persisted and returned on subsequent incident queries
- [ ] Given an incident with annotations, when `GET /api/v1/incidents?without_annotation=acknowledged` is called, then that incident is excluded from results
- [ ] Given an incident with annotations, when `GET /api/v1/incidents?with_annotation=acknowledged` is called, then that incident is included
- [ ] Given annotations on error groups, when `GET /api/v1/query?without_annotation=acknowledged` is called, then unannotated error groups are returned
- [ ] Given multiple consumers annotating the same incident, when annotations are queried, then all annotations coexist without conflict
- [ ] Given `mix test` runs, then annotation CRUD, query filtering, and multi-consumer coexistence are covered

## Notes
Architectural decision: Canary owns annotation *storage and query*. Consumers own annotation *semantics*. This avoids imposing a triage workflow while providing the shared ledger that multi-agent coordination requires.

Annotations are append-only facts ("agent X did Y at time T"), not mutable state. This sidesteps the crash-and-stuck-lease problem: if an agent crashes before annotating, the incident simply remains unannotated and can be picked up again.

Design reference: Sentry's `unresolved/ignored/resolved` is effectively a fixed-vocabulary annotation. We're generalizing this to arbitrary consumer-defined vocabulary.

Requires: new `annotations` schema/table, annotation controller, query filter extensions.
Feeds into: 010-ramp-pattern.md (triage loop).
Complements: 005-connect-a-service-workflow.md (onboarding can document annotation conventions).
