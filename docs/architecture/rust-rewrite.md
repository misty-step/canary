# Canary Rust Rewrite Architecture

This document is the working architecture map for the full Rust rewrite. It is
not a compatibility waiver: the Rust service must preserve Canary's agent-facing
contracts while moving correctness guards into Rust types, exhaustive enums, and
contract tests.

## Strategic Design Rules

- Deep modules own hard decisions. HTTP handlers route and translate; domain
  crates validate, classify, transition, persist, and emit typed outcomes.
- Wire contracts are stable product contracts. OpenAPI, RFC 9457 Problem
  Details, signed webhook headers, ID prefixes, and scoped API keys must remain
  compatible unless a migration document explicitly breaks them.
- SQLite remains a single-writer store until the product requirement changes.
  Rust must encode the writer boundary explicitly instead of hiding contention
  behind generic pools.
- No semantic wrappers around generic agents. Agent-facing replay, timelines,
  incidents, and summaries are deterministic data products.
- State machines stay pure. Persistence, webhooks, metrics, and logging consume
  typed effects returned by pure modules.

## Crate Layout

```text
crates/
  canary-core/      # typed IDs, health FSM, grouping, classification, incidents
  canary-http/      # RFC 9457, auth/scope wire behavior, OpenAPI parity helpers
  canary-store/     # SQLite schema, migrations, single-writer repository
  canary-ingest/    # validates payloads and commits grouped errors
  canary-events/    # timeline ledger and event fanout
  canary-workers/   # webhook delivery, retention, TLS scan, retry ledger
  canary-server/    # Axum router, app wiring, config, telemetry, shutdown
```

The first server crate now exists, but it is intentionally an adapter, not a
new product layer. `canary-server` mounts only the public unauthenticated routes
whose bodies are built by `canary-http::public`; it does not duplicate response
logic or introduce database, auth, or responder behavior.

The crate boundaries should stay deep:

- `canary-core` owns pure domain decisions and exposes typed outcomes.
- `canary-http` owns wire translation and compatibility helpers.
- `canary-store` will hide SQLite, migrations, and the single-writer boundary.
- `canary-ingest` will expose one high-level `ingest` operation rather than a
  scatter of validation, grouping, classification, and incident hooks.
- `canary-events` will make timeline append plus webhook fanout one committed
  operation, so callers cannot forget half of the product contract.

Avoid small crates or modules that only rename another layer. In the Phoenix
service, thin facades such as summary/status/report response builders are useful
locally but should not become Rust crate boundaries.

## Current Parity Anchors

- Endpoint map: `priv/openapi/openapi.json` and `lib/canary_web/router.ex`.
- Error body shape: `lib/canary_web/plugs/problem_details.ex`.
- Typed ID prefixes: `lib/canary/id.ex`.
- Pure health transitions: `lib/canary/health/state_machine.ex` and
  `test/canary/health/state_machine_test.exs`.
- Error grouping and classification: `lib/canary/errors/grouping.ex`,
  `lib/canary/errors/classification.ex`, and `lib/canary/errors/ingest.ex`.
- SQLite schema: `priv/repo/migrations/*.exs`.
- Webhook delivery contract: `lib/canary/workers/webhook_delivery.ex`.
- Footguns to encode, not rediscover: `CLAUDE.md`.

## Compatibility Rules

These details are easy for agents to break and should become golden tests before
the Rust server accepts production traffic:

- JSON request body limit remains 102400 bytes. `POST /api/v1/errors` also keeps
  its content-length preflight before JSON parsing.
- Problem Details bodies use `type`, `title`, `status`, `detail`, `code`,
  optional `request_id`, and flattened metadata. The `type` URL remains
  `https://canary.dev/problems/<dash-code>`.
- Authorization accepts exactly `Bearer <key>` after the prefix. Scopes remain
  `ingest-only`, `read-only`, and `admin`, with admin accepted everywhere.
- Rate limit policies remain `ingest: 100/60s`, `query: 30/60s`, and
  `auth_fail: 10/60s`. `retry_after` stays in the Problem Details body.
- Query windows remain the closed enum `1h`, `6h`, `24h`, `7d`, and `30d`.
- Cursor precedence remains `after` before `cursor` where both are accepted.
- Error ingest validation order remains required fields, context-size limit,
  then fingerprint validation.
- Truncation limits remain message 4096 bytes, stack trace 32768 bytes, and
  context 8192 bytes.
- Webhook headers remain `content-type`, `x-signature`, `x-event`,
  `x-delivery-id`, `x-webhook-version`, and `x-sequence`.
- Webhook delivery keeps stable `X-Delivery-Id` across retries, HMAC body
  signing as `sha256=<hex>`, four attempts, and backoff of 1, 5, 30, and 60
  seconds.
- Empty success responses remain HTTP 204 with no JSON body.

## First Implementation Slice

1. `canary-core::ids`: prefixed newtypes for `ERR`, `INC`, `TGT`, `MON`,
   `WHK`, `KEY`, `ANN`, `CHK`, `EVT`, and `DLV`.
2. `canary-core::health::state_machine`: pure transition function with typed
   states, events, thresholds, counters, and effects.
3. `canary-core::ingest::grouping`: grouping priority for client fingerprints,
   stack traces, and normalized message templates.
4. `canary-core::ingest::classification`: deterministic classification rules
   for category, persistence, and component.
5. `canary-http::problem_details`: RFC 9457 body compatible with the Phoenix
   implementation.
6. `canary-http::auth`: bearer-header extraction, scoped API-key authorization
   decisions, and Phoenix-compatible 401/403 Problem Details bodies.
7. `canary-http::public`: public unauthenticated endpoint contracts for
   `/healthz`, `/readyz`, and `/api/v1/openapi.json`, including unchanged
   OpenAPI bytes from `priv/openapi/openapi.json`.
8. `canary-server`: an Axum public-router adapter for `/healthz`, `/readyz`,
   and `/api/v1/openapi.json` that preserves status codes, content type, body
   bytes, and the absence of private routes.
9. `canary-http::webhooks`: HMAC-SHA256 signing, verification, and outbound
   webhook header construction for exact body bytes, including Phoenix parity
   fixtures for `sha256=<hex>`, `x-delivery-id`, `x-event`,
   `x-webhook-version`, and `x-sequence`.
10. `canary-store`: a single-writer SQLite boundary with ordered schema
    migrations ported from the Phoenix Ecto migrations, plus compatibility tests
    for table shape, defaults, indexes, FTS triggers, foreign keys, and
    open-incident uniqueness.
11. `canary-store::commit_error_ingest` and `canary-ingest`: transactional
    error persistence plus a deep ingest boundary that owns Phoenix validation
    order, truncation, grouping, classification, and the single store call.

This slice is deliberately small but aligned with the full rewrite: it moves
eleven existing contracts into Rust types and tests. The server crate is allowed
to know Axum, routing, and response conversion; it is not allowed to own product
decisions already expressed by `canary-core` or `canary-http`.

## Verification Expectations

Every migration slice needs both Rust-native tests and parity tests against the
Phoenix behavior until the replacement is complete:

- Unit tests cover pure behavior in `canary-core`.
- Golden tests lock wire bodies, headers, IDs, HMAC signatures, and OpenAPI
  responses.
- Property tests cover normalization, parser round trips, ID parsing, and state
  machine invariants.
- Database tests run migrations into a temporary SQLite database and assert both
  schema shape and repository behavior.
- HTTP tests exercise the same endpoint, auth, and error cases in the OpenAPI
  contract.
- The repo gate calls Rust from `./bin/validate`: fast validation runs
  `cargo fmt --all --check` and `cargo check --workspace --all-targets --locked`;
  deterministic validation runs clippy and tests; advisory validation runs
  `cargo audit`.

## Next Slices

1. Wire `POST /api/v1/errors` through `canary-server`: content-length preflight,
   scoped auth, JSON decoding, `canary-ingest`, 201 response shape, and RFC 9457
   validation/413/500 Problem Details.
2. Add post-commit effect handling for new-class/regression events without
   making broadcast, incident correlation, or webhook enqueue failures fail the
   ingest response.
3. Port webhook ledger and delivery after ingest is stable; preserve
   `X-Delivery-Id`, `X-Signature`, `X-Event`, `X-Webhook-Version`, and
   `X-Sequence`.
4. Add compatibility checks against a migrated Phoenix fixture database before
   any production traffic moves to the Rust server.
