# CLAUDE

Self-hosted observability for agent-driven infrastructure. Rust + SQLite.

## Monorepo Layout

- Rust workspace: core service, HTTP contracts, SQLite store, ingest, workers,
  and Axum runtime under `crates/`.
- TypeScript SDK: `clients/typescript/`.
- External responders consume Canary's signed webhooks and query APIs. They are
  not part of this repo.

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

## Invariants

- RFC 9457 Problem Details for all error responses.
- Summaries are deterministic templates. No LLM on the request path.
- No service names hardcoded. Targets, monitors, and webhooks are configured at
  runtime via API.
- Seeds only create a bootstrap API key. No hardcoded targets.

## Deploy

```bash
flyctl deploy --app canary-obs --remote-only

# Nuclear reset (stop first, then delete, then restart)
flyctl machines stop <id> --app canary-obs
flyctl ssh console --app canary-obs -C "rm -f /data/canary.db /data/canary.db-wal /data/canary.db-shm"
flyctl machines start <id> --app canary-obs
```

Bootstrap API key is logged on first boot only. Grep for
`"Bootstrap API key:"` and store it.

## Responder Boundary

Canary is the observability substrate. It owns error ingest, health checks,
incident correlation, timelines, query APIs, and signed generic webhooks.

- Repo mutation, issue creation, and LLM triage live outside Canary.
- Consumers should subscribe via generic webhooks and query back into Canary for
  context.
- Treat webhook payloads as stable product contracts, not app-specific glue.

## Self-Monitoring

Canary reports its own errors through the Rust direct-ingest path, no HTTP
loopback.
