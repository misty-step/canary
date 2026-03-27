# Connect A Service Workflow

Priority: high
Status: ready
Estimate: M

## Goal
Let a solo operator connect a new service to Canary through one opinionated path instead of stitching together API keys, target setup, snippets, and report queries by hand.

## Non-Goals
- Replace the existing REST API surface
- Build a generic multi-tenant onboarding system
- Ship runtime-specific SDKs for every language in this item

## Oracle
- [ ] Given a fresh Canary instance, when an operator follows the new flow, then they can create an API key, register a target, and obtain exact error-reporting snippets from one place
- [ ] Given the flow is completed for a sample service, when the operator opens `/api/v1/report` and the dashboard, then the new service appears without extra setup steps outside Canary docs or UI
- [ ] Given the onboarding surface exists, when `mix test` runs, then coverage proves the key creation, target registration, and onboarding composition behavior stays green

## Notes
This is the highest-signal gap from the grooming session. Canary already has the core primitives, but the user journey is still an assembly exercise spread across API routes and README examples.
