# Canary — Agent Router

Self-hosted observability substrate for AI agents (not humans). Rust + SQLite + Litestream → S3-compatible object storage. Misty Step production runs on a dedicated DigitalOcean host at **`https://canary.mistystep.io`** under `canary.service`; the Docker container is **`canary`** and the durable host mount is **`/var/lib/canary`**. v1: single region, one Docker image, one SQLite file. Read `VISION.md` for the product north star before changing product scope, responder boundaries, or agent-facing surfaces. Load-bearing footguns are inlined below (this file is now the single canonical harness doc — `CLAUDE.md` is a symlink to it).

## Stack & boundaries

| Layer | Owns | Path |
|---|---|---|
| Core service | HTTP surface, error ingest, health probing, correlation, timelines, queries, signed webhooks | `crates/canary-*` |
| TypeScript SDK | JS/TS client; `tsup` build + `vitest` | `clients/typescript/` |
| CI module | Single source of truth for the gate (Dagger TS) | `dagger/` |
| Bin scripts | Operator API — validate, dagger, bootstrap, DR | `bin/` |
| Backlog | 100% on Powder (repo `canary`) — cards, claims, status; no repo-local ticket files | `powder` CLI / MCP |

(Rust workspace crates cover core service, HTTP contracts, SQLite store, ingest, workers, and the Axum runtime.)

External responders (e.g. bitterblossom) consume Canary's signed webhooks and query back. They live **outside this repo** and are not part of it.

## Ground-truth pointers (files that ARE the contract)

- **Agent-facing API contract:** `GET /api/v1/openapi.json` — source under `priv/openapi/`; `info.x-agent-guide` embeds the canonical replay guide.
- **Router + auth pipelines:** `crates/canary-server/src/lib.rs`, `crates/canary-server/src/server_auth.rs`, and route-family modules under `crates/canary-server/src/`.
- **Error-response shape (RFC 9457):** `crates/canary-http/src/problem_details.rs`.
- **Pure state machine:** `crates/canary-core/src/health/state_machine.rs` — transition logic has no side effects.
- **Webhook delivery ledger:** `crates/canary-server/src/webhook_delivery.rs` + `crates/canary-workers/src/webhooks.rs` (stable `X-Delivery-Id` across retries).
- **Health runtime:** `crates/canary-server/src/target_probes.rs`, `crates/canary-server/src/monitor_overdue.rs`, and `crates/canary-workers/src/health.rs`.
- **Alerter trio:** `crates/canary-server/src/webhooks.rs` and `crates/canary-workers/src/webhooks.rs` own signing, cooldown, and circuit decisions.
- **Query read models:** `crates/canary-store/src/query.rs` and `crates/canary-server/src/query_routes.rs`.
- **Ingest path:** `crates/canary-ingest/src/lib.rs`; Canary self-reporting uses Rust direct ingest, no HTTP loopback.
- **Schemas:** `crates/canary-store/src/schema.rs` — all custom string PKs keep stable prefixes (`ERR-`/`INC-`/`WHK-`/`MON-nanoid`).

Prefer these over re-deriving from the code base.

## Invariants (hard rules)

- **Single writer.** All writes go through `canary-store::Store`; the production runtime shares one writable SQLite store behind the server lock.
- **`canary_core::health::state_machine::transition` stays pure.** No side effects. Verified by table-driven tests.
- **Summaries are deterministic templates.** No LLM on the request path.
- **RFC 9457 Problem Details** for every error response.
- **Scoped API keys** (`ingest-only` / `read-only` / `admin`) enforced at the router. See `docs/api-key-rotation.md`.
- **Responder boundary.** Canary owns ingest/health/correlation/timelines/queries/webhooks. Repo mutation, issue creation, and LLM triage live downstream. Consumers subscribe via generic webhooks and query back into Canary for context. Webhook payloads are stable product contracts, not app-specific glue.
- **No service names hardcoded.** Targets, monitors, and webhooks are configured at runtime via API. Seeds create only the bootstrap API key.
- **Target vs Monitor:** `Target` = HTTP URL probed on an interval. `Monitor` = check-in watcher for non-HTTP runtimes (desktop apps, cron, workers). Modes `schedule` or `ttl`. See `docs/non-http-health-semantics.md`.

## Gate contract

**`./bin/validate` IS the gate.** Do not invent parallel vocabulary.

| Invocation | Behavior | Wired to |
|---|---|---|
| `./bin/validate` | → `./bin/dagger check` (deterministic lanes + secrets scan) | manual run |
| `./bin/validate --fast` | → `dagger call fast` (lint + core tests) | `.githooks/pre-commit` |
| `./bin/validate --strict` | → `dagger call strict` (full gate + advisories + optional `.codex/agents/*.toml` validation when present) | `.githooks/pre-push` |
| `./bin/validate --advisories` | live advisory scan only | manual run |
| `dagger call strict --source=../candidate` | Hosted CI in `pull_request_target` immutable control plane (trusted base checkout at `.ci/trusted/`, candidate at `.ci/candidate/`) | `.github/workflows/ci.yml` |
| `bin/dr-status --host "$CANARY_SSH_HOST"` | Production Litestream status through the host/container boundary | manual pre-deploy and incident check |
| `bin/dr-restore-check --host "$CANARY_SSH_HOST"` | Non-destructive restore into container tmpfs | manual pre-deploy and DR drill |

**Package gates inside strict:**
- Rust workspace: format, check, clippy (`-D warnings`), tests.
- `clients/typescript/`: typecheck, coverage, build.
- Operator scripts: entrypoint, DR, and dogfood audit shell tests.
- Production image: Docker build + `/healthz` and `/readyz` smoke.

`bin/dagger` refuses CLI version drift from `dagger.json`. Do not hand-edit `.github/workflows/ci.yml` from a PR branch — the workflow lives outside the candidate diff per `docs/ci-control-plane.md`.

## Known-debt map

| Area | File(s) | Issue |
|---|---|---|
| **canary-010 Ramp pattern** (blocked, XL, north-star) | Powder | Blocked ~3.5 months on nonexistent bitterblossom artifacts; unblock-or-kill proposal via canary-932 child 5. |
| **canary-020 Adminifi HTTP surface verification** (blocked, S) | Powder | Upstream Adminifi HTTP surface stability. |
| **canary-063 Triage contract hardening** (backlog, XL, P1) | Powder | Durable webhook cooldown, dispatch budgets, claim-gated delivery. |
| **canary-064 Trustworthy release/upgrade** (backlog, L, P1) | Powder | Rescoped 2026-07-09: release restore → canary-931, pullable image → canary-934; likely closeable after both. |
| **canary-065 Runtime hardening** (backlog, L, P1) | Powder | bcrypt child superseded by canary-930; proxy-header trust invariant, DO backup posture, witness cadence truth. |
| **canary-066 Consolidation and archaeology deletion** (backlog, XL, P2) | Powder | Worker lifecycle QUINT unification (webhook_delivery is the divergent fifth), oban_jobs rename (gated on prod DB restamp), ValidationErrors relocation / canary-ingest fold, fixture WAL ignore. |
| Recurring footgun surface | Footguns section below + Rust store/runtime/schema modules | Every remediation here must cite the footgun list and extend it when new failure modes appear. |
| **canary-930 Request-path concurrency** (ready, P0) | Powder | bcrypt-under-store-lock root cause (live-reproduced), /readyz spiral, mutex poisoning, monitor_overdue scan, oban_jobs growth. Consolidates the slow-API/500 cards. |
| **canary-931 Release pipeline restore** (ready, P0) | Powder | Releaser App secrets missing (releases hard-down), zero GitHub releases, version disagreement, npm SDK unpublished. |
| **canary-932 Coordination loop in anger** (ready, P0) | Powder | CLI/MCP read-half parity (incident get, timeline cursor, drill-downs, parity guard) + dogfood claims on real incidents. |
| **canary-933 Gate proves live behavior** (ready, P1) | Powder | Latency floor, seeded-volume + concurrency rehearsal, post-deploy gate, Rust coverage ratchet, diff-scoped strict. Absorbs 914/972. |
| **canary-934 De-Fly ops surface** (ready, P1) | Powder | DO Spaces backups, DR transport seam, pullable image, deploy/witness cutover, DR runbook rewrite. Coordinates with do-migration-104/105. |
| **canary-935 /ui first-class** (ready, P1) | Powder | Vendored fonts, graceful degradation, read contract, UI smoke, mobile-first. Folds 067/068/915 intent. |
| **canary-936 Service-bound reads + redaction corpus** (ready, P0) | Powder | Unbound read keys read cross-service rich context; four-regex redaction. 048 successor; ADR-gated scope model. |

The backlog is 100% on Powder (operator ruling 2026-07-09): query with `powder` CLI or the powder MCP (`list_cards`/`list_ready` with `repo: canary`); claim before work; done cards carry shipping evidence. The former `backlog.d/` tree was deleted — its history lives in git.

## Outer loop

User-ratified composition: **`/settle → /refactor → /code-review → merge`.** Master keeps one squash commit per PR via `gh pr merge --squash`; PR title + body become that commit. Conventional-with-scope prefix on the PR title / squash subject (`feat(health):`, `fix(ci):`, `refactor(query):`, `chore(governance):`, `docs(ops):`, `build:`). Narrow test idiom: `cargo test -p <crate> <test_name> --locked`.

## Self-monitoring

Canary reports its own errors through the Rust runtime direct-ingest path, no HTTP loopback. Query Canary itself (`GET /api/v1/query?service=canary&window=1h`) for post-deploy signal.

## Cold-agent readiness proof

`bin/canary-readiness-proof --json` is the one discoverable entrypoint proving a cold agent can inspect and operate this instance: doctor, mcp-manifest/mcp-server, dogfood discovery, and `bin/validate --fast`, ending in a redacted receipt. See `docs/agent-inspection-cli.md#cold-agent-readiness-proof`.

## Deploy (operational crib)

```bash
export CANARY_ENDPOINT=https://canary.mistystep.io
export CANARY_SSH_HOST=<operator-ssh-target>
ssh "$CANARY_SSH_HOST" sudo systemctl is-active canary.service
ssh "$CANARY_SSH_HOST" sudo docker inspect canary --format '{{.Image}} {{.State.Status}}'
bin/dr-status
bin/dr-restore-check
curl -fsS "$CANARY_ENDPOINT/healthz"
curl -fsS "$CANARY_ENDPOINT/readyz"
```

There is no provider auto-deploy workflow. Production promotion is an explicit
immutable-image update on the dedicated host, followed by the live proof above;
see `docs/upgrade-and-rollback.md`.

**Nuclear reset (human-gated, do NOT automate).** The invariant, in order:
**stop `canary.service`** (SQLite must release its WAL/SHM handles) → verify no
`canary` container remains → restore or remove `canary.db*` only while the
writer is stopped and the durable `/var/lib/canary` mount is verified → restart
the service so `bin/entrypoint.sh` restores from the Litestream replica. Never
delete `/data/canary.db*` through `docker exec` against the running container.
See `docs/backup-restore-dr.md`.

Bootstrap API key logged once on first boot — inspect `sudo docker logs canary`
on the host. If missed, use the supported `canary-server mint-key` path from
`docs/self-host-docker.md`; the original cannot be re-shown.

## Footguns

- **Single SQLite writer.** Production writes go through one `canary-store::Store`
  instance behind the server lock. Do not add hidden write pools or long-held
  store locks around network work.
- **Schema ownership.** `crates/canary-store/src/schema.rs` is the Rust schema
  source. `Store::migrate` must fail closed on partial existing schemas before
  stamping `user_version`.
- **Custom string IDs.** Stable prefixes (`ERR-`, `INC-`, `EVT-`, `WHK-`,
  `MON-`) are product contracts. Keep ID generation in `canary-core::ids`.
- **State machines stay pure.** Health transitions live in
  `canary-core::health::state_machine`; persistence, webhooks, metrics, and
  logging consume typed outcomes outside the pure transition logic.
- **Outbound HTTP egress.** Target probes and webhook delivery are server-side
  requests. Public egress validation belongs in `canary-server::egress`; tests
  that intentionally use loopback must opt in explicitly.
- **Webhook delivery jobs.** Claimed jobs must always be completed as succeeded,
  retry, or discarded, including runtime errors and panics. Never leave
  `executing` rows stranded.
- **Readiness is live.** `/readyz` must query the writable store each request.
  Do not replace it with static process state.
- **SQLite WAL and `rm -f`.** Deleting the DB while the app is running does
  nothing useful because SQLite WAL keeps file handles open. Stop the machine
  before destructive maintenance.
- **Retention prune lock time.** Retention deletes share the single writer with
  ingest, probes, and webhook delivery. Keep pruning in bounded batches and
  release the store lock between batches.
- **Rate limiter locality.** Rate limits are process-local fixed-window buckets.
  Do not claim fleet-wide rate limiting without adding a shared limiter.
- **No CPU-bound work under the store lock.** bcrypt (and any other expensive
  compute) must never run while holding the single-writer store mutex. The
  2026-07-09 groom live-reproduced the failure: per-request bcrypt verify under
  the lock serialized the whole service (~230 ms staircase per concurrent
  client, 7.5 s at ~30 clients) and put `/readyz` in the same queue.
- **Request path must not poison the writer mutex.** Workers wrap store work in
  `catch_unwind`; request handlers do not. One panic while holding the std
  `Mutex<Store>` makes every subsequent authenticated request 500 until
  restart. Contain panics or use a non-poisoning lock.
- **One egress oracle.** There is exactly one public-destination filter for
  outbound HTTP; probe and webhook paths must share it. Hand-maintained copies
  drift (the IPv4-mapped-address rejection landed in the probe copy only).

This list is load-bearing — every remediation in the Known-debt map above must cite it and extend it when new failure modes appear.
</content>
