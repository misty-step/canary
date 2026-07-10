# Architecture

Canary is a single Rust service deployed as one Docker image. It owns agent
observability ingest, health probing, timeline/query read models, and signed
webhook delivery. It does not own downstream repo mutation, issue creation, or
LLM triage.

## Runtime Topology

```text
canary-server
├── Axum router
│   ├── public routes: /healthz, /readyz, /api/v1/openapi.json
│   ├── ingest routes: errors and monitor check-ins
│   ├── read routes: query, report, timelines, incidents, delivery lookup
│   └── admin routes: keys, targets, monitors, webhooks, annotations
├── shared Store behind one process-local write lock
├── target probe lifecycle worker
├── monitor overdue lifecycle worker
├── webhook delivery lifecycle worker
├── retention prune lifecycle worker
└── TLS expiry scan lifecycle worker
```

There are no service-to-service calls inside Canary. Self-reporting uses the
Rust direct-ingest path, not an HTTP loopback.

## Crate Map

| Crate | Owns |
|---|---|
| `canary-core` | Typed IDs, pure health transitions, grouping, classification, incident/action brief decisions |
| `canary-http` | RFC 9457 Problem Details, bearer/scope wire behavior, public response contracts, webhook header signing |
| `canary-store` | SQLite schema, single-writer repository, query read models, delivery ledger, retention commands |
| `canary-ingest` | Error ingest validation, grouping, persistence, and typed post-commit effects |
| `canary-workers` | Pure worker planning for webhook delivery and retry decisions |
| `canary-server` | Axum routing, auth/rate limits, runtime wiring, probe transports, lifecycle workers |

The TypeScript client in `clients/typescript/` is a maintained SDK surface. It
is not part of the server runtime.

## Persistence

Production uses one SQLite file at `/data/canary.db` with WAL mode. Litestream
replicates the file to S3-compatible object storage. Recovery is restore-based,
not failover.

All writes go through `canary_store::Store`. The production server shares one
writable store behind a process-local lock so SQLite's single-writer constraint
is explicit rather than hidden behind a pool.

Schema source lives in `crates/canary-store/src/schema.rs`. Custom string IDs
use stable prefixes such as `ERR-`, `INC-`, `WHK-`, and `MON-`.

## Request Flow

### Error Ingest

```text
POST /api/v1/errors
  -> bearer parsing and ingest/admin scope check
  -> route-family rate limit
  -> JSON/body validation
  -> canary-ingest::ingest
     -> deterministic grouping and classification
     -> Store transaction for errors, groups, service events, incidents
     -> typed post-commit effects
  -> WebhookEnqueueEffectSink creates delivery ledger rows and jobs
  -> 201 {id, group_hash, is_new_class}
```

### Health Probe

```text
target lifecycle due check
  -> Store selects active due targets
  -> target request builder validates URL, headers, and SSRF policy
  -> reqwest transport pins validated DNS addresses and disables proxy env
  -> Store records target_check and target_state transition
  -> HealthEventFanout emits state-change webhook effects
```

### Webhook Delivery

```text
due webhook job
  -> load stable delivery request from Store
  -> validate destination and pin resolved addresses
  -> send signed request with stable X-Delivery-Id
  -> update delivery ledger and retry/discard/delivered state
```

## Design Invariants

- Agent-facing API contract is `GET /api/v1/openapi.json`, sourced from
  `priv/openapi/`.
- Every error response uses RFC 9457 Problem Details.
- API keys are scoped as `ingest-only`, `read-only`, or `admin`; route families
  enforce scope before business logic.
- Health transition logic in `crates/canary-core/src/health/state_machine.rs`
  is pure and table-tested.
- Summaries are deterministic templates. No LLM runs on the request path.
- Webhook payloads and headers are product contracts for downstream responders.
- Outbound target probes and webhooks validate public HTTP(S) destinations and
  pin the DNS answers they validated.
- The server has no human dashboard. Agents and operators read through the same
  query/report/status APIs.

## Operations

`./bin/validate` is the canonical local gate. It delegates to the pinned Dagger
module and covers the Rust workspace, TypeScript SDK, operator scripts,
production image smoke, secrets scanning, and advisory checks in strict mode.

The Misty Step production deployment uses a dedicated DigitalOcean host. The
`canary.service` systemd unit owns exactly one Docker container named `canary`,
binds its durable volume at `/var/lib/canary`, and exposes the process only
through Caddy at `https://canary.mistystep.io`. The Dockerfile builds the Rust
`canary-server` binary and `bin/entrypoint.sh` wraps Litestream restore and
replication before executing it. Generic operators can use the same image and
storage contract through `docs/self-host-docker.md`.
