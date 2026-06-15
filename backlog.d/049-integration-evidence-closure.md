# Close integration evidence and capture gaps

Priority: P1 · Status: pending · Estimate: XL

## Goal
Close the residual evidence gaps after `040` by making `canary integrate` unable
to overclaim verification: apply patches, enroll resources, deploy-smoke,
prove service-specific readback, and route browser capture through a safe
relay/public-ingest design.

## Oracle
- [ ] Given `canary integrate apply --project <path> --service <name> --production-url <url> --platform vercel|fly --receipt <path>` runs in a supported app, then it patches code, creates target/key/monitor/webhook resources as requested, writes platform env through an approved secret handoff, runs a deployed smoke, sends a synthetic event or check-in, confirms service-specific query readback, and writes a verified receipt only after all required evidence exists.
- [ ] Given only target/key creation succeeds, then integration status remains partial and the receipt cannot be marked verified.
- [ ] Given webhooks exist for other services only, then integration status does not count `webhook_configured=true` for the current service.
- [ ] Given a browser app needs client capture, then the generated integration uses a safe relay or public-ingest design rather than exposing a secret API key in `NEXT_PUBLIC_*`.
- [ ] Given Fly and Vercel projects are inspected, then env-name audit parity exists for production and preview/staging where the platform supports it.
- [ ] Given MCP users want integration tools, then an installable MCP wrapper over the CLI manifest exists and is smoke-tested.

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
3. Add Fly/Vercel env audit parity and approved secret-handoff adapters.
4. Design and implement safe browser capture through relay or public-ingest tokens.
5. Add `integrate apply` orchestration over patch, enroll, platform env, smoke, synthetic signal, and readback.
6. Ship an installable MCP wrapper over the CLI manifest.
