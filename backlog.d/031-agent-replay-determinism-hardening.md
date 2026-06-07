# Agent replay determinism hardening

Priority: high
Status: ready
Estimate: M

## Goal
Make agent replay and health probing fail explicitly at contract boundaries instead of silently accepting malformed cursors, unsafe target cadence, or unverifiable boot state.

## Non-Goals
- Add multi-region probing, consensus health checks, or HA semantics.
- Add log aggregation, tracing, or LLM triage.
- Change the timeline replay model or webhook payload semantics.
- Move repo mutation, issue creation, or downstream repair work into Canary.

## Oracle
- [ ] Malformed cursor values on agent-facing pagination endpoints return RFC 9457 `422 validation_error` responses instead of silently falling back to the first page or a legacy hash path. Cover at least `/api/v1/query` with `cargo test -p canary-server malformed_query_cursor --locked`.
- [ ] Health target creation rejects intervals that cannot safely schedule jittered checks. The lower bound is documented in the target schema/OpenAPI contract, and a controller test proves the invalid interval never spawns a checker.
- [ ] Persisted target methods cannot crash `Canary.Health.Probe.check/1`; invalid methods are rejected before persistence or converted into an explicit probe error in a focused unit test.
- [ ] Boot-time migration or seed failure cannot leave `/readyz` reporting healthy against an unverifiable schema. The chosen behavior is either fail-fast startup or readiness-gated failure, covered by a narrow test or release-task assertion.
- [ ] `./bin/validate --fast` is green on the branch.

## Notes

**Why now.** The current strategic direction is stronger autonomous consumers: cold-start, replay, act, annotate. That loop depends on deterministic boundary behavior. If a cursor is malformed, if a target interval can crash the checker scheduler, or if boot hides migration failure, the agent receives plausible but unreliable state.

**Evidence from grooming.**

- Rust replay and health code has shipped the malformed-cursor, unsafe-cadence,
  invalid-method, and boot-failure regressions listed below; keep this item
  open only until the evidence is folded into the active Rust contract suite.

**Rust production-path evidence.**

- Rust `/api/v1/query?service=...&cursor=bogus` now returns the standard
  RFC 9457 `422 validation_error` cursor problem instead of replaying the first
  page. `canary-store` preserves legacy group-hash cursor support behind a
  cursor-aware `ErrorGroupQueryError`, and the Axum adapter maps only
  service/error-class query cursor failures to the public problem shape.
- `1ab64a8` proves unsupported persisted target methods become explicit Rust
  `connection_error` target checks without opening transport.
- `184c5a3` proves Rust admin target creation, target interval update, and
  service onboarding reject sub-second target cadences before persistence or
  lifecycle commands; `priv/openapi/openapi.json` advertises the 1000ms lower
  bound.
- The Rust boot path in `CanaryServer::boot` fails before serving routes when
  store open, migration, or first-boot seed fails; keep the narrow regression
  test with this item until the Phoenix oracle is retired.

**Responder-boundary check.** This is Canary-side substrate hardening only: ingest/health/query/readiness contracts. Consumers still own triage and repair decisions.

**Lane.** Lane 2 (contract + observability). Pairs with #030: #030 makes the agent contract explicit; #031 makes malformed contract inputs fail deterministically.
