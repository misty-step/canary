# OpenAPI spec and agent integration guide

Priority: high
Status: done
Estimate: M

## Goal
Provide a machine-consumable API contract (OpenAPI 3.1 spec) for all Canary endpoints, plus a prose guide documenting the canonical agent integration pattern.

Agents should be able to discover and use the API without human mediation — feed the spec to an LLM or SDK generator and get a working client.

## Non-Goals
- Auto-generation from code (write the spec by hand, it's the contract)
- Hosted Swagger UI (serve the YAML/JSON at a well-known path, clients render it themselves)
- Tutorials or walkthroughs — spec + pattern reference only

## Oracle
- [x] Given `GET /api/v1/openapi.json`, then a valid OpenAPI 3.1 spec is returned describing all endpoints, params, request/response schemas, auth, and error shapes (RFC 9457)
- [x] Given the spec, when fed to an OpenAPI validator (`swagger-cli validate`), then it passes
- [x] Given the spec, when an agent reads it, then every endpoint's parameters, response schemas, and error codes are fully described (no `additionalProperties: true` escape hatches)
- [x] Given a prose section in the spec (`x-agent-guide` or `info.description`), then the webhook-notification + timeline-replay pattern is documented: webhook fires → agent wakes → polls timeline from last cursor → processes events → annotates
- [x] Given the guide, then crash-recovery is described: poll from last persisted cursor → catch up → resume
- [x] Given the guide, then `after` vs `cursor` precedence is documented

## Notes
Deferred from 002-timeline-agent-polling.md — the timeline API is shipped but undocumented.

The key insight: webhooks are fire-and-forget notifications. Timeline is the durable, queryable event log. Agents should never depend solely on webhook delivery for correctness.

Serve the spec at `GET /api/v1/openapi.json` so agents can self-discover. The
shipped implementation keeps the endpoint inside router/controller invariants
while still serving a hand-written JSON file from `priv/`.

Feeds into: 010-ramp-pattern.md (the ramp pattern's polling loop needs this documented).

As of 2026-04-01 audit: 3 hand-rolled clients (linejam, chrondle, adminifi) + 1 SDK
consumer (volume) with 1/7 adoption. All three external reviewers (Thinktank, Codex,
Gemini) agreed the OpenAPI spec is the forcing function for client convergence —
contract-first, not SDK-first. Only webhook sig verification and cursor replay are
worth standardizing as handcrafted helpers.

## What Was Built
- Published a hand-written OpenAPI 3.1 document at `GET /api/v1/openapi.json` via an explicit public controller route that serves `priv/openapi/openapi.json`.
- Documented the canonical agent contract in `info.x-agent-guide`: webhook wake-up hints, timeline replay from the last persisted cursor, crash recovery, and `after` vs `cursor` precedence.
- Covered the contract with a dedicated endpoint test that verifies public availability, guide presence, self-discovery, rate-limit error coverage, and the absence of `additionalProperties: true` escape hatches.
- Updated the main README API section so operators and SDK authors can discover the public contract entrypoint directly.
