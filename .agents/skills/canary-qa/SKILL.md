---
name: canary-qa
description: |
  QA Canary changes by exercising the real running service, not just tests.
  Canary is a self-hosted agent observability service: an Axum HTTP API
  (ingest/health/query/webhooks) over one SQLite writer, plus a `canary`
  inspection CLI and a TypeScript SDK. "Tests pass" is not QA. Use when: "QA
  this", "verify the feature", "smoke test canary", "check the API", "test
  canary". Trigger: /canary-qa.
argument-hint: "[http|cli|sdk|webhooks|route|feature]"
---

# canary-qa

QA in Canary means driving the surface that changed against a running server.
`./bin/validate` (→ `./bin/dagger check`) is the deterministic gate — Rust
fmt/clippy/tests, TS typecheck/coverage/build, operator-script tests, Docker
`/healthz`+`/readyz` smoke, secrets scan. It is **necessary but not sufficient**:
it cannot prove the live ingest→query→correlate→webhook write path fires, that
summaries read correctly, or that the CLI talks to a real instance. The gate
needs Docker/Colima up (`colima start --runtime docker`); `./bin/validate --fast`
= lint+core tests; `cargo test -p <crate> <test> --locked` runs one test.

## Surfaces

| Changed area | Surface | QA path |
|---|---|---|
| `crates/canary-server/**`, `canary-ingest`, `canary-store`, `canary-core` | HTTP service | Start server, replay real requests (ingest→query→report), check contract shape + RFC 9457 error path |
| `crates/canary-cli/**` | `canary` CLI | Point at the local server, run the affected command + `doctor`; check exit code AND payload |
| `crates/canary-workers/**`, `webhooks.rs`, `target_probes.rs`, `monitor_overdue.rs` | Workers | Drive the async side effect (health transition / webhook delivery) via the write-path rehearsal; confirm the delivery ledger, not just "it ran" |
| `clients/typescript/**` | TS SDK | `npm run typecheck && npm test && npm run build` in `clients/typescript/` |

## Start local runtime

The Rust binary does **not** auto-load `.env`; pass a local DB path (default is `/data/canary.db`, unwritable locally). Prebuilt debug binaries live in `target/debug/`.

```sh
# HTTP server → binds 0.0.0.0:4000 (PORT default 4000)
CANARY_DB_PATH=./canary_dev.db cargo run -p canary-server
# stderr: "canary-server listening on 0.0.0.0:4000"
# first boot only, once: "Bootstrap API key: <raw>"  (CANARY_DISCLOSE_BOOTSTRAP_KEY defaults true)

# Get a scoped key anytime (must target the SAME db); prints raw key to stdout:
CANARY_DB_PATH=./canary_dev.db cargo run -q -p canary-server -- mint-key --scope admin
# scopes: admin | read-only | ingest-only
```

- Public routes need no key: `/healthz`, `/readyz`, `/api/v1/openapi.json`.
- Everything else needs a scoped key: ingest (`POST /api/v1/errors`, `/check-ins`), read (query/report/timeline/health-status), admin (targets/monitors/webhooks/keys/onboarding).
- Fallback: port taken → `PORT=4400 CANARY_DB_PATH=./canary_dev.db cargo run -p canary-server`.

## HTTP service QA (the write path tests can't fake)

```sh
KEY=$(CANARY_DB_PATH=./canary_dev.db cargo run -q -p canary-server -- mint-key --scope admin)
curl -s localhost:4000/healthz   # {"status":"ok"}
curl -s localhost:4000/readyz    # {"status":"ready"} — hits the writable store each call
# ingest → 201 {"id":"ERR-...","group_hash":"sha256...","is_new_class":true}
curl -s -X POST localhost:4000/api/v1/errors -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' \
  -d '{"service":"qa-smoke","error_class":"QaError","message":"qa probe","severity":"error"}'
# read it back — the error must appear + the summary must read correctly
curl -s "localhost:4000/api/v1/query?service=qa-smoke&window=1h" -H "Authorization: Bearer $KEY"
curl -s "localhost:4000/api/v1/report?window=1h"                 -H "Authorization: Bearer $KEY"
# error path (RFC 9457): bad auth → 401 application/problem+json, NOT a bare 500
curl -s -i "localhost:4000/api/v1/query?service=qa-smoke" -H "Authorization: Bearer nope" | head
```

Check: `201` with an `ERR-` id, the error surfaces in query/report, the deterministic summary text is sane, bad auth returns `application/problem+json`.

## CLI QA

Default endpoint is prod `https://canary-obs.fly.dev` — **override it or you QA production.**

```sh
export CANARY_ENDPOINT=http://localhost:4000 CANARY_ADMIN_KEY=$KEY
./target/debug/canary summary --window 1h    # or ./bin/canary (rebuilds)
./target/debug/canary doctor
./target/debug/canary errors qa-smoke --json
```

## Workers / webhook delivery QA (async side effects)

Use the purpose-built live rehearsal — it creates disposable target/monitor/webhook resources, drives ingest + check-in, verifies the delivery ledger + query/report/timeline, then cleans up. Needs outbound egress (default webhook → `httpbingo.org`); a `localhost` webhook is rejected by egress validation unless `ALLOW_PRIVATE_TARGETS=true`.

```sh
CANARY_ENDPOINT=http://localhost:4000 CANARY_API_KEY=$KEY \
  bin/canary-write-path-rehearsal --no-dr-status --json
```

## Gotchas

- **`./bin/validate` green ≠ the service works.** Fixtures/tests are canned; live ingest→webhook is not covered. Drive the running server for any request-path/worker change.
- **`.env` is not loaded by the binary** — export vars or inline them; forgetting `CANARY_DB_PATH` makes the server try `/data/canary.db`.
- **CLI defaults to production** — always set `--endpoint`/`CANARY_ENDPOINT` for local QA.
- **Single SQLite writer** — `mint-key` and the running server must point at the same `CANARY_DB_PATH`.
- **Dagger gate needs Docker/Colima** running, or `./bin/validate` fails before doing anything.

## Report

Return: **verdict** (PASS / FAIL / UNVERIFIED) · exact commands run · surfaces exercised (HTTP / CLI / SDK / workers) · artifacts inspected (response bodies, rehearsal receipt, delivery ledger) · what was NOT covered (e.g. "no webhook delivery — HTTP only") and whether a post-deploy self-signal (`GET /api/v1/query?service=canary`) covers it.
