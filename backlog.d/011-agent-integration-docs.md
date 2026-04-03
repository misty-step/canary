# OpenAPI spec and agent integration guide

Priority: high
Status: ready
Estimate: M

## Goal
Provide a machine-consumable API contract (OpenAPI 3.1 spec) for all Canary endpoints, plus a prose guide documenting the canonical agent integration pattern.

Agents should be able to discover and use the API without human mediation — feed the spec to an LLM or SDK generator and get a working client.

## Non-Goals
- Auto-generation from code (write the spec by hand, it's the contract)
- Hosted Swagger UI (serve the YAML/JSON at a well-known path, clients render it themselves)
- Tutorials or walkthroughs — spec + pattern reference only

## Oracle
- [ ] Given `GET /api/v1/openapi.json`, then a valid OpenAPI 3.1 spec is returned describing all endpoints, params, request/response schemas, auth, and error shapes (RFC 9457)
- [ ] Given the spec, when fed to an OpenAPI validator (`swagger-cli validate`), then it passes
- [ ] Given the spec, when an agent reads it, then every endpoint's parameters, response schemas, and error codes are fully described (no `additionalProperties: true` escape hatches)
- [ ] Given a prose section in the spec (`x-agent-guide` or `info.description`), then the webhook-notification + timeline-replay pattern is documented: webhook fires → agent wakes → polls timeline from last cursor → processes events → annotates
- [ ] Given the guide, then crash-recovery is described: poll from last persisted cursor → catch up → resume
- [ ] Given the guide, then `after` vs `cursor` precedence is documented

## Notes
Deferred from 002-timeline-agent-polling.md — the timeline API is shipped but undocumented.

The key insight: webhooks are fire-and-forget notifications. Timeline is the durable, queryable event log. Agents should never depend solely on webhook delivery for correctness.

Consider serving the spec at `GET /api/v1/openapi.json` so agents can self-discover. No Phoenix dependency needed — just a static JSON file served from priv.

Feeds into: 010-ramp-pattern.md (the ramp pattern's polling loop needs this documented).

As of 2026-04-01 audit: 3 hand-rolled clients (linejam, chrondle, adminifi) + 1 SDK
consumer (volume) with 1/7 adoption. All three external reviewers (Thinktank, Codex,
Gemini) agreed the OpenAPI spec is the forcing function for client convergence —
contract-first, not SDK-first. Only webhook sig verification and cursor replay are
worth standardizing as handcrafted helpers.
