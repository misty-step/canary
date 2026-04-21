# Webhook delivery ledger and idempotency

Priority: high
Status: done
Estimate: M

## Goal
Make webhook delivery reliable enough for agent consumers that act on events.
Stable delivery IDs across retries, a persistent delivery ledger for operator
visibility, and a documented consumer idempotency contract.

## Non-Goals
- Full message queue semantics (Kafka, AMQP) — Oban remains the delivery engine
- Consumer-side SDK for dedup — document the contract, consumers implement it
- Webhook replay API in this item — ledger enables it, separate item delivers it

## Oracle
- [x] Given a webhook delivery is retried by Oban, when the consumer inspects `x-delivery-id`, then the ID is identical across all attempts for the same logical event
- [x] Given a webhook delivery succeeds or is discarded, when an operator queries the delivery ledger, then the attempt count, final status, and timestamps are visible
- [x] Given circuit breaker or cooldown suppresses a delivery, when the ledger is queried, then the suppression reason is recorded (not silently dropped)
- [x] Given the agent integration guide (011) exists, then the idempotency contract (deduplicate on `x-delivery-id`, treat webhooks as wake-up hints, replay from timeline for correctness) is documented

## Notes
Identified as the #1 architectural gap by all three external reviewers (Thinktank, Codex, Gemini) during the 2026-04-01 audit.

Current state: `x-delivery-id` is generated per attempt via `Nanoid.generate()` in
`webhook_delivery.ex:81`, so retries produce different IDs. Cooldown key is
`#{webhook.id}:#{event}`, which can suppress distinct events of the same type.
Circuit breaker and cooldown state are ETS-only and reset on restart.

Codex recommended expanding to a full delivery ledger + stable IDs + DLQ. This item
covers the ledger and stable IDs; replay/DLQ is a follow-up.

Load-bearing for the triage sprite (bb/011) and the ramp pattern north star (010).

## What Was Built
- Added a persistent `webhook_deliveries` ledger with one row per logical delivery, status transitions, attempt counters, timestamps, and reason metadata.
- Generated `DLV-*` delivery IDs at enqueue time, reused them across retries, and derived deterministic fallback IDs for legacy jobs that were already queued without a delivery ID.
- Recorded cooldown and circuit-open suppressions in the ledger instead of silently dropping them, and tightened cooldown keys so distinct same-type payloads do not collide behind `webhook_id:event`.
- Added `GET /api/v1/webhook-deliveries` with validated filters and cursor pagination for operator visibility into delivery history.
- Updated webhook contract docs in `README.md` and `spec.md` to document stable delivery IDs, wake-up-hint semantics, replay guidance, and the delivery ledger endpoint.

## Workarounds
- Local test verification required `MIX_ENV=test mix ecto.reset` once after renaming the new ledger reason column during implementation, because the uncommitted test SQLite schema had already been migrated with the earlier column name.
