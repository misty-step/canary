# Agent contract safety pass

Priority: high
Status: done
Estimate: M

## Goal

Make Canary's OpenAPI contract explicit enough that an autonomous
consumer can infer required key scope, trust deterministic summaries, and
replay webhook delivery context without out-of-band human interpretation.

## Non-Goals

- Build a general AI-agent tracing platform or ingest arbitrary LLM spans.
- Add structured log aggregation; `PRINCIPLES.md` keeps logs out of v1.
- Add GitHub, Slack, PagerDuty, or other opinionated integrations.
- Move repo mutation, issue creation, or LLM triage into Canary.
- Replace the existing timeline replay model; webhooks stay wake-up hints.

## Oracle

- [x] Every authenticated OpenAPI operation declares the required Canary API
      key scope in a machine-readable field, and a contract test fails when
      `crates/canary-server/src/lib.rs` route wiring and `priv/openapi/openapi.json`
      drift.
- [x] Every JSON response schema either includes a deterministic `summary`
      field or is listed in a small documented exception table with the reason
      an agent does not need synthesis for that response.
- [x] Primary agent entrypoints (`/api/v1/report`, `/api/v1/timeline`,
      `/api/v1/incidents/{id}`, `/api/v1/check-ins`, and annotation writes)
      have operation-level descriptions or `x-agent-guidance` metadata that
      says when to call them, what to trust in the response, and what to do
      next.
- [x] `GET /api/v1/webhook-deliveries/{delivery_id}` returns one ledger row
      by stable `X-Delivery-Id`, including event type, status, attempt count,
      response metadata, payload identity, and enough context for an agent to
      reconcile a disputed or failed delivery.
- [x] `info.x-agent-guide.webhook_contract` documents the canonical recovery
      path: dedupe by `X-Delivery-Id`, read the delivery row for diagnostics,
      and replay durable state through `/api/v1/timeline` before acting.
- [x] `info.x-agent-guide` includes a `cold_start` path for agents with no
      saved cursor: start at `/api/v1/report`, follow truncation/cursor hints,
      then switch to timeline replay for durable state.
- [x] The annotation write-back convention is documented and typed without
      prescribing downstream behavior: OpenAPI exposes stable `action` values
      for `triaged`, `fix-proposed`, `fix-verified`, and `noise-dismissed`,
      plus expected opaque `metadata` keys for consumers that choose to use
      them.
- [x] `cargo test -p canary-server openapi --locked`
      covers scope annotations, summary completeness, and the new delivery-id
      lookup contract.
- [x] `./bin/validate --fast` green on the branch that introduces the pass.

## Notes

**Why now.** The current repo is strong on agent-facing primitives, but the
contract is weaker than the runtime. The Rust router enforces ingest, read,
and admin permissions, while `openapi.json` exposes a global `bearerAuth`
scheme and a scope enum but does not bind required scopes to operations. The
spec also says agents should dedupe by `X-Delivery-Id` and replay through the
timeline, but the delivery ledger is list-only from the contract's point of
view.

**2026-06-11 grooming evidence.** Rust now exposes
`GET /api/v1/webhook-deliveries/{delivery_id}` and the checked-in
`info.x-agent-guide.webhook_contract` names that diagnostic path. The remaining
gap is the operation-level contract: an inspection of `priv/openapi/openapi.json`
found 40 operations, zero `x-canary-required-scope` entries, and zero
`x-agent-guidance` entries. Keep this ticket focused on scope metadata, summary
completeness, cold-start guidance, and annotation write-back conventions.

**Research signal.** Current agent-observability work is converging on rich
structured traces, tool-call context, state transitions, operation replay, and
bounded summaries. Canary should not chase that entire product category. The
elegant Canary move is narrower: make the substrate's existing bounded
summaries, timeline, scoped keys, annotations, and webhook ledger
machine-verifiable enough for downstream agents to operate safely.

**Execution sketch.**

1. Add a small OpenAPI extension such as `x-canary-required-scope` to every
   authenticated operation, using Rust route registration as source-of-truth
   evidence.
2. Extend the primary operation descriptions and add structured
   `x-agent-guidance` only where it helps an autonomous consumer choose the
   next call.
3. Extend the existing Rust OpenAPI/route contract tests to assert scope and
   summary completeness.
4. Add the delivery lookup route under the read permission and expose the
   response schema in OpenAPI.
5. Update `info.x-agent-guide` to make cold-start, annotation write-back, and
   delivery-id diagnostics subordinate to timeline replay.

**Responder-boundary check.** Canary only exposes diagnostic context and
generic replay semantics. Consumers still decide what to do with that context.

**Lane.** Lane 2 (contract + observability). Depends on #011 and #012, both
already done. Ships independently.
