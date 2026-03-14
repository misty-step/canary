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
- No HA — Fly machine restart = brief monitoring gap
- Restore-based DR — Litestream backup, not automatic failover
- At-least-once webhooks — use `X-Delivery-Id` to deduplicate

Pretending these limitations don't exist is worse than documenting them. Users can make informed decisions.

## 5. Deep Modules, Simple Interfaces

Following Ousterhout's *A Philosophy of Software Design*:

- `Ingest.ingest/1` — one function hides validation, grouping, persistence, webhook dispatch
- `StateMachine.transition/4` — pure function, no side effects, table-driven tests
- `Grouping.compute_group_hash/1` — three strategies behind one interface
- ETS services (`RateLimiter`, `DedupCache`, `CircuitBreaker`, `Cooldown`) — GenServer lifecycle + ETS internals hidden behind `check/2`, `mark/1`, `open?/1`

The interface should be simple even when the implementation is complex.

## 6. Deterministic Over Probabilistic

Summaries are template strings, not LLM output. Grouping is sha256 hashing, not clustering. State transitions are a finite state machine, not a classifier.

When the system that monitors your infrastructure is itself unpredictable, you have two problems. Canary's behavior is fully determined by its inputs.

## 7. Code Is a Liability

Every line fights for its life. The right answer is often "don't build that."

- No MCP server (API + CLI + skill files are sufficient)
- No dashboard (agents are the UI)
- No built-in integrations (webhooks are generic)
- No multi-tenant support (internal tool)
- No log aggregation (structured errors only)

Features that aren't built can't break.

## 8. Separation of Stateful and Stateless

GenServers own stateful, cadence-sensitive work (probe scheduling, consecutive failure tracking, flap detection). Oban owns stateless, retry-oriented work (webhook delivery, retention pruning, TLS scanning).

This isn't arbitrary — it follows from the nature of the work. Probes need monotonic scheduling and in-memory counters. Webhooks need exponential backoff and persistence across restarts.

## 9. Design for Migration, Don't Build for It

The data model includes `region` fields for future multi-region support. Ecto abstraction preserves the Postgres migration path. But v1 doesn't build multi-region or HA — it just doesn't block it.

Design decisions that keep future options open cost nothing. Building for hypothetical requirements costs everything.
