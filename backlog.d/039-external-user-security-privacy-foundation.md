# Build the external-user security and privacy foundation

Priority: P0
Status: ready
Estimate: XL

## Goal
Make Canary safe for arbitrary users and applications by adding tenant/project isolation, privacy-by-default ingestion, service-bound public ingest credentials, and auditable authority boundaries across reads, writes, webhooks, and automation handoff.

## Oracle
- [ ] API keys, targets, monitors, errors, incidents, annotations, webhook subscriptions, and delivery rows carry durable tenant/project/service ownership; read and admin routes can only return rows authorized for the caller.
- [ ] Public browser ingest uses service-bound DSNs or equivalent public credentials that cannot impersonate another service, environment, or tenant.
- [ ] Server-side ingestion applies default secret/PII redaction before persistence, with project-level scrub rules, raw payload retention controls, and regression tests for common token/header/request-body leaks.
- [ ] Webhook subscriptions are scoped by tenant/project/service/event, with timestamped signatures, replay-window validation, secret rotation, and least-privilege responder read tokens.
- [ ] Rate limits and quotas are enforceable beyond process-local fixed windows before any hosted multi-tenant claim is made.
- [ ] OpenAPI, CLI/MCP, SDK docs, and integration snippets stop calling raw `NEXT_PUBLIC_CANARY_API_KEY` exposure safe until the bound public-ingest model exists.

## Children
1. Introduce tenant/project/service ownership schema and migrate existing single-org rows into a bootstrap tenant.
2. Enforce row-level authorization on query, report, timeline, target, monitor, incident, annotation, webhook, and delivery routes.
3. Replace public browser ingest keys with service-bound public DSNs or constrained write tokens.
4. Add server-side redaction defaults, project scrub policy, retention controls, and raw-payload opt-in.
5. Harden webhook auth with timestamped signatures, replay windows, rotation, scoped subscriptions, and scoped responder read keys.
6. Add shared rate limits, quotas, and abuse controls for browser ingest and unauthenticated/invalid-key traffic.
7. Update OpenAPI, CLI, SDK, and docs to express the new authority model.

## Notes
- Evidence: `AGENTS.md` describes v1 as single org; `crates/canary-http/src/auth.rs` models only operation scopes; `crates/canary-store/src/schema.rs` stores service strings without tenant/project columns; `crates/canary-ingest/src/lib.rs` accepts caller-supplied `service`; `clients/typescript/src/index.ts` defaults `scrubPii` to false.
- Security lane found this is the gating boundary before external arbitrary-app onboarding.
- This is a foundation epic, not a hardening grab bag. Tenancy, privacy defaults, scoped browser ingest, webhook scoping, and quotas must land as one coherent product boundary.
