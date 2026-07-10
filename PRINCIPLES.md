# Principles

Design principles that govern every decision in Canary. When in conflict, earlier principles take precedence.

## 1. Agent-First

The primary consumer of every API response is an LLM with a finite context window.

- Every query response includes a `summary` field with natural-language synthesis
- Response payloads are bounded (max 50 groups, cursor pagination)
- Error data is pre-aggregated (error_groups rollup table, not raw event scans)
- No information requires clicking through a dashboard to understand

## 2. Single Deployable

One Docker image. One database file. One config. No microservices, no message queues, no external service dependencies beyond S3 for backups.

Complexity that lives in your infrastructure is complexity you operate at 3am. Canary is a single process that either works or doesn't. There is nothing to debug between services.

## 3. Broadcast, Don't Prescribe

Canary fires HMAC-signed webhooks on state transitions. It does not know or care what consumers do with them.

No GitHub integration. No Slack integration. No Discord integration. No PagerDuty integration. Consumers wire their own behavior. This keeps Canary simple and consumers unconstrained.

## 4. Honest Constraints

v1 limitations are documented, not hidden:

- Single region — health checks reflect connectivity from deployment region only
- No HA — host or container restart = brief monitoring gap
- Restore-based DR — Litestream backup, not automatic failover
- At-least-once webhooks — use `X-Delivery-Id` to deduplicate

Pretending these limitations don't exist is worse than documenting them. Users can make informed decisions.

## 5. Deep Modules, Simple Interfaces

Following Ousterhout's *A Philosophy of Software Design*:

- `canary_ingest::ingest` — one function hides validation, grouping, persistence, webhook dispatch
- `canary_core::health::state_machine::transition` — pure function, no side effects, table-driven tests
- `canary_core::ingest::grouping` — three hash strategies (fingerprint, stack, template) behind one `Grouping` struct
- `canary_store::Store` — the single-writer SQLite boundary hides migrations, schema validation, and all persistence behind one owned handle

The interface should be simple even when the implementation is complex.

## 6. Deterministic Over Probabilistic

Summaries are template strings, not LLM output. Grouping is sha256 hashing, not clustering. State transitions are a finite state machine, not a classifier.

When the system that monitors your infrastructure is itself unpredictable, you have two problems. Canary's behavior is fully determined by its inputs.

## 7. Code Is a Liability

Every line fights for its life. The right answer is often "don't build that."

- No dashboard (agents are the UI; the operator dashboard was deleted, see `docs/operator-dashboard-removal.md`)
- No built-in integrations (webhooks are generic)
- No multi-tenant support (single-tenant binary is the product)
- No log aggregation (structured errors only)

Features that aren't built can't break. The MCP stdio server and CLI JSON envelope exist because they are thin adapters over the same HTTP API — not a second product surface.

## 8. Separation of Stateful and Stateless

In-process worker threads own stateful, cadence-sensitive work (target probes, monitor-overdue evaluation, retention pruning, TLS scanning) through dedicated `*Lifecycle` runtime structs in `canary-server`. Webhook delivery leases durable job rows from `canary-store` and drains them with backoff.

This isn't arbitrary — it follows from the nature of the work. Probes need monotonic scheduling and in-memory counters. Webhook delivery needs persistence across restarts and at-least-once semantics with `executing`-lease recovery.

## 9. Design for Migration, Don't Build for It

The data model is SQLite (WAL, one explicit writer, `user_version` schema stamping) — small, durable, and Litestream-replicated. The store's hand-rolled migrations are fail-closed on partial existing schemas before stamping. But v1 doesn't build clustering or HA — it just doesn't block a future move to a different store if scale ever demands it.

Design decisions that keep future options open cost nothing. Building for hypothetical requirements costs everything.
