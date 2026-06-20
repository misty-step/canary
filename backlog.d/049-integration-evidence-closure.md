# Close integration evidence and capture gaps

Priority: P1 · Status: pending · Estimate: XL

## PRD Summary
- User: agents integrating Canary into deployed applications and later trusting the receipt.
- Problem: integration status can still overclaim when only partial setup exists, webhooks are global rather than service-specific, synthetic readback is missing, or browser capture would expose credentials.
- Goal: make `canary integrate` unable to mark coverage verified until deployed service-specific evidence proves health, ingest/check-in, webhook, readback, and safe capture.
- Why now: #040 shipped the integration loop and #046 shipped value receipts; the remaining issue is receipt truth, not basic onboarding.
- UX enabled: an agent can run one apply/status loop and know whether a service is actually covered or exactly why it is partial.
- Deliverable type: working code, MCP wrapper, receipts, and smoke evidence.
- Success signal: strict integration status refuses verified coverage unless synthetic service-specific evidence is fresh and replayable.

## Product Requirements
- P0: `verified` requires synthetic ingress or check-in plus service-specific query/timeline readback.
- P0: webhook coverage is service-specific; a webhook for another service cannot satisfy the current service.
- P0: stale registry or stale integration receipt evidence cannot produce a verified coverage verdict.
- P0: browser capture depends on the public-ingest or relay contract from #048 and never exposes secret API keys.
- P1: provide `integrate apply` orchestration across patch, enroll, secret handoff, deploy smoke, synthetic signal, readback, and receipt finalization.
- P1: ship an installable MCP wrapper over the CLI manifest with a smoke proof.
- Non-goals: weakening manual review of code patches, writing raw secret values to logs, or making Canary mutate downstream repos through server routes.

## Technical Design
- Chosen architecture: keep the CLI as the integration orchestrator; receipts are durable local evidence, while Canary remains the server-side source for readback and service state.
- Files/systems touched: `crates/canary-cli/src/lib.rs`, `crates/canary-cli/src/main.rs`, `bin/dogfood-inventory`, `bin/dogfood-audit`, MCP manifest artifacts, TypeScript SDK/browser capture helpers, and integration docs.
- Data/control flow: discover local app, plan patch/enroll, apply reviewed local changes, perform approved secret handoff, create target/key/webhook/monitor resources, emit synthetic event/check-in, read query/timeline/status back for the same service, then write verified receipt.
- Build/check boundary: unit tests catch receipt semantics; shell tests catch strict audit/dogfood behavior; MCP smoke catches wrapper install and one tool invocation.
- ADR decision: required for the browser public-ingest/relay design if #048 has not already recorded it.
- ADR-style invariants: receipts describe evidence, not intent; service-specific readback is mandatory; secret values never appear in receipt output.
- Design X vs Y: do not treat target/key creation as verification; target/key creation is setup, while deployed synthetic readback is proof.

## Goal
Close the residual evidence gaps after `040` by making `canary integrate` unable
to overclaim verification: apply patches, enroll resources, deploy-smoke,
prove service-specific readback, and route browser capture through a safe
relay/public-ingest design.

## Oracle
- [ ] Given `canary integrate apply --project <path> --service <name> --production-url <url> --platform vercel|fly --receipt <path>` runs in a supported app, then it patches code, creates target/key/monitor/webhook resources as requested, writes platform env through an approved secret handoff, runs a deployed smoke, sends a synthetic event or check-in, confirms service-specific query readback, and writes a verified receipt only after all required evidence exists.
- [ ] Given only target/key creation succeeds, then integration status remains partial and the receipt cannot be marked verified.
- [ ] Given webhooks exist for other services only, then integration status does not count `webhook_configured=true` for the current service.
- [ ] Given an integration receipt is stale or only backed by registry text, then strict dogfood and integration status fail or report partial rather than verified coverage.
- [ ] Given synthetic ingress/check-in cannot be read back through query, timeline, status, or dogfood value for the same service, then the receipt cannot be marked verified.
- [ ] Given a browser app needs client capture, then the generated integration uses a safe relay or public-ingest design rather than exposing a secret API key in `NEXT_PUBLIC_*`.
- [ ] Given Fly and Vercel projects are inspected, then env-name audit parity exists for production and preview/staging where the platform supports it.
- [ ] Given MCP users want integration tools, then an installable MCP wrapper over the CLI manifest exists and is smoke-tested.

## Verification System
- Claim: integration status and receipts cannot overclaim service coverage.
- Falsifier: a receipt is verified after target/key creation only, webhook coverage comes from another service, browser capture exposes a secret key, or stale evidence passes strict mode.
- Driver: CLI unit tests, `bin/dogfood-inventory --strict --json` fixtures, `bin/canary integrate status/plan/apply --json` fixtures, and MCP wrapper smoke.
- Grader: coverage stays partial until service-specific synthetic readback exists; receipts redact secrets and carry fresh evidence timestamps; MCP smoke returns a valid tool envelope.
- Evidence packet: integration receipt fixtures plus a production or local throwaway app transcript.
- Cadence: fast/unit tests for semantics; strict or dedicated integration smoke before marking the ticket done.

## Notes
Why: `040` shipped a universal integration/enrollment engine, but the
agent-readiness lane found remaining overclaiming defects rather than a missing
onboarding concept. `plan` still leaves platform env and webhook work manual,
Fly env-name listing is not implemented, receipts can overclaim after
target/key creation, global webhook presence can satisfy coverage, synthetic
readback is not mandatory, and browser capture still lacks a safe public
relay/DSN story.

Sequence after `045`-`048` and the Bitterblossom `055` canary/incident
responder template: close the operator/value/safety/responder loops first so
integration receipts know what they must prove.

## Children
1. Tighten receipt semantics so `verified` requires synthetic ingest/check-in and readback.
2. Make webhook coverage service-specific.
3. Fail strict/status on stale registry or stale receipt evidence when it is the only proof.
4. Add Fly/Vercel env audit parity and approved secret-handoff adapters.
5. Implement safe browser capture only after #048 defines the relay or public-ingest authority.
6. Add `integrate apply` orchestration over patch, enroll, platform env, smoke, synthetic signal, and readback.
7. Ship an installable MCP wrapper over the CLI manifest.
