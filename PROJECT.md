# Project

Current state, delivery phases, and operational context for Canary.

## Status

**Live.** Deployed to https://canary-obs.fly.dev on Fly.io (iad region).

| Component | Status |
|-----------|--------|
| Health checker (3 targets) | Running |
| Error ingestion API | Live |
| Query API with summaries | Live |
| Webhook broadcasting | Live |
| API key auth | Live |
| Retention pruning (Oban cron) | Scheduled |
| TLS expiry scanning (Oban cron) | Scheduled |
| Litestream backup | Not configured (no S3 bucket yet) |

## Phased Delivery

### Phase 1: Health Checker — COMPLETE

Replaces Uptime Robot. Smallest useful surface.

- [x] DynamicSupervisor + GenServer-per-target health checkers
- [x] State machine (unknown → up → degraded → down → flapping)
- [x] SSRF guard (block private/loopback/metadata IPs)
- [x] Shared Finch connection pool
- [x] TLS certificate expiry extraction
- [x] Probe history persistence
- [x] `GET /api/v1/health-status` with summary
- [x] `GET /api/v1/targets/:id/checks`
- [x] Webhook dispatch on state transitions
- [x] Target management API (add/remove/pause/resume/list)
- [x] API key auth (bcrypt, constant-time comparison)
- [x] SQLite with WAL + PRAGMAs
- [x] `/healthz` and `/readyz`
- [x] Deployed to Fly.io, monitoring cadence + heartbeat + overmind
- [ ] **Gate:** 1 week parallel operation with Uptime Robot

### Phase 2: Error Ingestion — COMPLETE

Replaces Sentry capture.

- [x] `POST /api/v1/errors` with validation + payload limits
- [x] Error groups rollup table (upsert on ingest)
- [x] Client fingerprint override
- [x] Per-API-key rate limiting (ETS token bucket)
- [x] Webhook dedup cache
- [x] Webhooks on `error.new_class` and `error.regression`
- [x] Rate limiting + RFC 9457 error responses
- [ ] Client library (Elixir)
- [ ] **Gate:** One service migrated off Sentry SDK

### Phase 3: Query API — COMPLETE

Agent-first differentiator.

- [x] `GET /api/v1/query` with service filter, window, cursor pagination
- [x] `GET /api/v1/errors/:id` with full detail
- [x] Natural-language `summary` in all responses
- [x] Response size bounded (max 50 groups)
- [x] Query rate limiting (30/min per key)
- [ ] Claude Code skill file
- [ ] Reference clients (Python, TypeScript, Go)
- [ ] **Gate:** Agent queries and acts on error data in real debugging session

### Phase 4: Migration & Decommission — NOT STARTED

- [ ] All services migrated to client library
- [ ] Sentry SDKs removed
- [ ] Uptime Robot closed
- [ ] Sentry downgraded/closed

## Infrastructure

| Resource | Detail |
|----------|--------|
| Fly.io app | `canary-obs` |
| Region | `iad` (US East) |
| Machine | `shared-cpu-1x`, 512MB RAM |
| Volume | `canary_data`, 1GB, encrypted |
| Database | SQLite at `/data/canary.db` |
| Domain | `canary-obs.fly.dev` |

## Secrets

| Secret | Location |
|--------|----------|
| `SECRET_KEY_BASE` | Fly.io secrets |
| `API_KEY_SALT` | Fly.io secrets |
| Bootstrap API key | Logged on first boot (store securely) |

## Monitored Targets

| Target | URL | Interval |
|--------|-----|----------|
| cadence-api | https://cadence.fly.dev/api/health | 60s |
| heartbeat | https://heartbeat.fly.dev/api/health | 60s |
| overmind | https://overmind.fly.dev/api/health | 120s |

## Next Steps

1. Configure Litestream S3 backup (DO Spaces)
2. Run parallel with Uptime Robot for 1 week
3. Build Elixir client library
4. Migrate first service off Sentry
5. Build Claude Code skill file for agent queries
