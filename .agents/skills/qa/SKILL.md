---
name: qa
description: |
  Browser-based QA, exploratory testing, evidence capture, and bug reporting.
  Drive running applications and verify they work — not just that tests pass.
  Use when: "run QA", "test this", "verify the feature", "exploratory test",
  "check the app", "QA this PR", "capture evidence", "manual testing",
  "scaffold qa", "generate qa skill".
  Trigger: /qa.
argument-hint: "[url|route|feature|scaffold]"
---

# /qa (canary)

Canary is an API-first observability substrate for agents. The "browser" here
is `curl` (or `gh api`) against `https://canary-obs.fly.dev` for live, or
`http://localhost:4000` after `mix phx.server`. There is no human dashboard —
operators read the same API agents do.

QA effectiveness is about proving **the feature works end-to-end against live
canary**, not about proving `mix test` is green. Those are different claims.

## Execution Stance

You are the executive orchestrator.
- Keep test scope, severity classification, and final pass/fail call on the lead model.
- Delegate route execution and evidence capture to focused subagents when the
  matrix is wide (e.g. exercise all four read endpoints across three scopes
  in parallel).
- Use independent verification when the same agent captured the evidence.

## Tests Pass vs Feature Works

Two separate gates. Both required before shipping.

| Claim                | Command                                                                                     |
|----------------------|---------------------------------------------------------------------------------------------|
| Tests pass (fast)    | `./bin/validate --fast` (pre-commit equivalent)                                             |
| Tests pass (strict)  | `./bin/validate --strict` (pre-push equivalent, includes live advisories + role TOMLs)      |
| Narrow iteration     | `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3`                      |
| Feature works (prod) | `curl` matrix in [API + Webhook QA Matrix](#api--webhook-qa-matrix) against `canary-obs`    |
| Feature works (local)| Same matrix against `http://localhost:4000` with a `mix phx.server` in another terminal     |
| Dogfood gate         | `bin/dogfood-audit --strict` (owned HTTP service manifest vs. live targets)                 |

Coverage thresholds inside strict: core **81%** (see `mix.exs` `test_coverage`),
`canary_sdk/` **90%**. Never lower either to pass — Red Line.

## Routing

| Intent                               | Action                                                                     |
|--------------------------------------|----------------------------------------------------------------------------|
| "scaffold qa", "generate qa skill"   | Read `references/scaffold.md` and follow it                                |
| Run QA after an ingest/query change  | Use the [API + Webhook QA Matrix](#api--webhook-qa-matrix)                 |
| Run QA after a health/state change   | Use the [Health + Monitor QA Matrix](#health--monitor-qa-matrix)           |
| Run QA after a webhook change        | Use the [Webhook Delivery QA Matrix](#webhook-delivery-qa-matrix)          |
| Verify live owned services           | Run `bin/dogfood-audit --strict` (see `docs/networked-service-dogfooding.md`) |

## Environment Setup

```bash
# Live (production canary-obs)
export CANARY_ENDPOINT=https://canary-obs.fly.dev
export CANARY_INGEST_KEY=...   # scope: ingest-only
export CANARY_READ_KEY=...     # scope: read-only
export CANARY_ADMIN_KEY=...    # scope: admin

# Local (mix phx.server on :4000)
export CANARY_ENDPOINT=http://localhost:4000
# Bootstrap API key is logged once on first boot:
#   flyctl logs --app canary-obs | grep "Bootstrap API key:"
# For local: same — watch the `mix phx.server` boot log.
```

Scope enforcement is a Red-Line invariant: `ingest-only` must 403 on
`/api/v1/query`, `read-only` must 403 on `POST /api/v1/errors`, etc. Every
QA pass must prove this at least once.

## API + Webhook QA Matrix

Primary QA surface. Every structured response must include a natural-language
`summary` field — verify it on every call. No LLM on the request path, so
`summary` is a deterministic template (hot modules: `lib/canary/query.ex`,
`lib/canary/timeline.ex`, `lib/canary/incidents.ex`).

```bash
# 1. Ingest — POST /api/v1/errors with ingest-only key → 201 + ERR-...
curl -sS -X POST "$CANARY_ENDPOINT/api/v1/errors" \
  -H "Authorization: Bearer $CANARY_INGEST_KEY" \
  -H "Content-Type: application/json" \
  -d '{"service":"qa-smoke","error_class":"RuntimeError","message":"qa smoke","severity":"error"}' \
  | tee /tmp/qa-canary/ingest.json
# Assert: .id matches /^ERR-/, .group_hash present, .is_new_class boolean.

# 2. Query — GET /api/v1/query with read-only key → bounded payload + summary.
curl -sS "$CANARY_ENDPOINT/api/v1/query?service=qa-smoke&window=1h" \
  -H "Authorization: Bearer $CANARY_READ_KEY" | jq '.summary, .total_errors'

# 3. Report — GET /api/v1/report combines health + errors + incidents + transitions.
curl -sS "$CANARY_ENDPOINT/api/v1/report?window=1h" \
  -H "Authorization: Bearer $CANARY_READ_KEY" | jq '.summary, .status, (.incidents | length)'

# 4. Timeline — canonical events (same payload shape as webhook deliveries).
curl -sS "$CANARY_ENDPOINT/api/v1/timeline?service=qa-smoke&window=24h&limit=50" \
  -H "Authorization: Bearer $CANARY_READ_KEY" | jq '.summary, (.events | length)'

# 5. OpenAPI — GET /api/v1/openapi.json is the agent contract (public).
curl -sS "$CANARY_ENDPOINT/api/v1/openapi.json" | jq '.info."x-agent-guide" | length'

# 6. Scope enforcement — ingest key on read endpoint must 403 (RFC 9457).
curl -sS -o /tmp/qa-canary/scope-violation.json -w "%{http_code}\n" \
  "$CANARY_ENDPOINT/api/v1/query?service=qa-smoke&window=1h" \
  -H "Authorization: Bearer $CANARY_INGEST_KEY"
# Assert: 403, body is RFC 9457 Problem Details
# (.type, .title, .status, .detail, .instance all present).
```

## Health + Monitor QA Matrix

`Canary.Health.StateMachine.transition/4` is pure — transitions are
deterministic. For health features the authoritative trace is a table-driven
test (`test/canary/health/state_machine_test.exs`), but live QA still needs
to confirm the wiring end-to-end.

```bash
# Health snapshot with NL summary (3 up, 1 degraded, etc.)
curl -sS "$CANARY_ENDPOINT/api/v1/health-status" \
  -H "Authorization: Bearer $CANARY_READ_KEY" | jq '.summary'

# Non-HTTP monitor check-in (schedule or ttl mode) advances state without
# creating error groups. Crash/exception telemetry still goes to POST /api/v1/errors.
curl -sS -X POST "$CANARY_ENDPOINT/api/v1/check-ins" \
  -H "Authorization: Bearer $CANARY_INGEST_KEY" \
  -H "Content-Type: application/json" \
  -d '{"monitor_id":"MON-...","status":"alive"}' | jq '.status, .state'
```

Owned-service dogfood verification runs out-of-band:

```bash
bin/dogfood-audit --strict --window 1h
# Reads priv/dogfood/owned_services.json; compares to live targets +
# per-service report/query output. Strict mode exits non-zero on any
# missing/duplicated/wrong-URL target.
# Reference: docs/networked-service-dogfooding.md.
```

## Webhook Delivery QA Matrix

Webhook payloads are a stable product contract (responder boundary —
consumers like bitterblossom live downstream). HMAC-SHA256 signed, at-least-
once, dedupe on `X-Delivery-Id`. Circuit breaker opens after 10 failures,
probes every 5 min; cooldown is 5 min per webhook+event type; restart
`canary-obs` to reset ETS state.

```bash
# 1. Subscribe (admin scope).
curl -sS -X POST "$CANARY_ENDPOINT/api/v1/webhooks" \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"url":"https://httpbin.org/post","events":["health_check.down","error.new_class"]}' \
  | tee /tmp/qa-canary/webhook.json
# Capture: .id (WHK-...), .secret (one-time; used to verify signatures).

# 2. Test delivery — sends canary.ping, non-business, NOT timelined.
WHK_ID=$(jq -r '.id' /tmp/qa-canary/webhook.json)
curl -sS -X POST "$CANARY_ENDPOINT/api/v1/webhooks/$WHK_ID/test" \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" | jq '.delivery_id'

# 3. Verify delivery ledger (read scope).
curl -sS "$CANARY_ENDPOINT/api/v1/webhook-deliveries?webhook_id=$WHK_ID&limit=5" \
  -H "Authorization: Bearer $CANARY_READ_KEY" \
  | jq '.deliveries[] | {id, event, status, attempts, signature_header, delivery_id}'
# Assert: X-Delivery-Id is stable across retries; HMAC-SHA256 signature header
# present; retry attempts dedupe on the same delivery_id.
```

Hot modules to correlate: `lib/canary/webhooks/delivery.ex`,
`lib/canary/alerter/circuit_breaker.ex`, `lib/canary/alerter/cooldown.ex`,
`lib/canary/alerter/signer.ex`.

## Evidence Capture

Write everything to `/tmp/qa-canary/` (create it first). For every finding,
keep the paired `curl` request + response so a subagent can reproduce cold.

| Area change       | Capture                                                                                                       |
|-------------------|---------------------------------------------------------------------------------------------------------------|
| API / router      | Request (method, path, headers with keys redacted, body) + response (status, body, latency)                   |
| Webhook delivery  | `X-Delivery-Id`, signature header, raw payload, attempt number, delivery ledger row                           |
| Health / state    | `transition/4` trace from `test/canary/health/state_machine_test.exs` + live `/api/v1/health-status` snapshot |
| Dogfood           | Full `bin/dogfood-audit --strict` stdout                                                                      |

Redact `CANARY_*_KEY`, webhook `secret`, and any bootstrap key from anything
that leaves `/tmp/`. Keys log once and once only on first boot — losing one
means re-bootstrapping.

## Invariants to Verify Every QA Pass

All of these are Red-Line product contracts — never mark a pass clean if one
fails:

- **RFC 9457 Problem Details** on every error response (`type`, `title`,
  `status`, `detail`, `instance`). Source: `lib/canary_web/problem_details.ex`.
- **Scoped API key enforcement** (`ingest-only` / `read-only` / `admin`)
  via router pipelines `:scope_ingest`, `:scope_read`, `:scope_admin` in
  `lib/canary_web/router.ex`. Cross-scope requests must 403 with Problem Details.
- **Natural-language `summary` field** on every structured read response
  (`/query`, `/report`, `/timeline`, `/health-status`, and onboarding).
- **No LLM on request path.** Response latency stays stable under load.
  Summaries are deterministic templates — identical inputs produce byte-
  identical summaries.
- **Webhook payloads** match the shape emitted by `/api/v1/timeline` for the
  same event. Timeline events are the canonical observability facts; the
  webhook is a wake-up hint with the same payload.

## Severity Classification

| Tier | Blocks ship? | Examples                                                                                 |
|------|--------------|------------------------------------------------------------------------------------------|
| P0   | Yes          | Scope bypass (ingest key accepted on read). Missing `summary`. Non-RFC 9457 error body. Webhook signature missing or wrong. Bootstrap key unrecoverable. Dogfood audit --strict exits non-zero on active service. |
| P1   | Before merge | Summary template regression (wrong pluralization, stale counts). Retry dedupe broken on `X-Delivery-Id`. Coverage drops below 81% (core) / 90% (sdk). |
| P2   | Log + backlog| Minor summary phrasing, extra target not in manifest.                                    |

Log P1/P2 to `backlog.d/` with the usual `#NNN` form (see `backlog.d/README.md`).

## Gotchas

- **"`mix test` green" is not QA.** Tests verify code paths. QA verifies the
  live contract — especially scope enforcement, Problem Details shape, and
  deterministic `summary` output.
- **Don't QA against a stale local DB.** If you used Canary locally for an
  earlier feature, reset: `mix ecto.reset` (the SQLite WAL makes `rm -f`
  a no-op on a live machine; on a Fly machine you must stop → ssh rm → restart
  per the `CLAUDE.md` runbook).
- **Don't skip `bin/dogfood-audit --strict` on health/target changes.**
  Owned HTTP services are the live proof that ingest + health + query wiring
  is intact end-to-end. Reference `docs/networked-service-dogfooding.md`.
- **Don't re-use an ingest key for read traffic just because it's handy.**
  That silently masks the exact scope-enforcement regression this skill is
  designed to catch.
- **Webhook retries are at-least-once — dedupe on `X-Delivery-Id`.** If a
  consumer test harness doesn't, retries will double-count.
- **`POST /api/v1/webhooks/:id/test` sends `canary.ping`**, which is
  explicitly non-business and not written to the timeline. Use real events
  (e.g. ingest a new error class) when you need to prove the full path.

## When a Project-Local Scaffold Is Needed

If first argument is `scaffold` → read `references/scaffold.md`. For the
shared fallback protocol and browser-tool reference, read
`references/browser-tools.md` and `references/evidence-capture.md`. This
skill deliberately assumes API-first; the fallback exists for the rare
cases where a non-API Canary surface (e.g. a new LiveView panel) needs its
own documented QA protocol.
