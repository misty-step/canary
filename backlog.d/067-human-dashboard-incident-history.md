# Human dashboard historical incident index

Priority: P2 · Status: pending · Estimate: M

## Goal
Give the human dashboard a first-class incident history surface instead of
deriving recent history from timeline events. The current v1 dashboard lists
open incidents from `GET /api/v1/incidents` and recent incident events from
`GET /api/v1/timeline`; that keeps the client thin, but it is not a complete
historical incident index.

## Oracle
- [ ] Given an operator opens the Incidents view, then open and resolved
      incidents come from one bounded incident-list read model with pagination.
- [ ] Given an incident is resolved, then it remains selectable without relying
      on a recent timeline event still being inside the requested window.
- [ ] Given a service-bound read key is used, then historical incidents are
      filtered by that service without leaking other services.
- [ ] Given the dashboard renders history, then it still fetches detail through
      `GET /api/v1/incidents/{id}` and does not reimplement incident state.

## Verification System
- Claim: humans can inspect all incident history through a thin dashboard
  client.
- Falsifier: an incident disappears from the dashboard once its timeline event
  ages out, or the browser reconstructs incident rows from raw event payloads.
- Driver: route tests for the new read model, OpenAPI scope tests, service-scope
  isolation tests, and light/dark dashboard screenshots with resolved history.
- Grader: response shape is deterministic, paginated, scoped, and backed by
  existing store incident state.
- Evidence packet: dashboard screenshot set plus route-test names in the PR.

## Notes
Filed from the v1 human dashboard lane. Do not add this by making timeline a
private data lake in the client; add the smallest read model that answers the
operator question directly.
