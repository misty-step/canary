# Architecture

Canary is a single Elixir/OTP application deployed as one Docker container. No microservices, no message queues, no external dependencies beyond Fly Tigris for backups.

## Supervision Tree

```
Canary.Application
├── Canary.Repo                      # SQLite write repo (pool_size: 1)
├── Canary.ReadRepo                  # SQLite read repo (pool_size: 4)
├── Canary.Release                   # Boot-time migrations + seeds (runs once)
├── CanaryWeb.Telemetry              # Phoenix telemetry
├── Phoenix.PubSub                   # Internal pub/sub
├── DNSCluster                       # Fly.io DNS-based clustering
├── Finch (Canary.Finch)             # Shared HTTP connection pool
├── Oban                             # Background job processing
│   ├── WebhookDelivery workers      # Retry-based webhook dispatch
│   ├── RetentionPrune workers       # Daily data pruning
│   └── TlsScan workers              # Daily TLS expiry checks
├── Canary.Errors.RateLimiter        # ETS token bucket (per-key)
├── Canary.Errors.DedupCache         # ETS webhook dedup window
├── Canary.Alerter.CircuitBreaker    # ETS per-subscription health
├── Canary.Alerter.Cooldown          # ETS per-group suppression
├── Registry (Canary.Health.Registry)# Process registry for checkers
├── Canary.Health.Supervisor         # DynamicSupervisor for checkers
├── Canary.Health.Manager            # Target lifecycle (CRUD → checker start/stop)
└── CanaryWeb.Endpoint               # Phoenix + Bandit HTTP server
```

## Module Map

```
lib/
  canary/
    application.ex          # Supervision tree
    release.ex              # Boot-time migrations + seeds
    id.ex                   # Nanoid-based ID generation
    auth.ex                 # API key generation, bcrypt hashing, verification
    query.ex                # Query API logic (reads from error_groups, targets)
    summary.ex              # Deterministic natural-language summaries
    json_logger.ex          # JSON structured logging for production
    seeds.ex                # First-boot seed data

    schemas/                # Ecto schemas (data shapes, changesets)
      error.ex              # Raw error events
      error_group.ex        # Rollup table (upserted on ingest)
      target.ex             # Health check target config
      target_check.ex       # Probe history
      target_state.ex       # Runtime state (separate from config)
      webhook.ex            # Webhook subscriptions
      api_key.ex            # API key metadata (hashed storage)

    health/                 # Health checking subsystem
      state_machine.ex      # Pure state machine (no side effects)
      checker.ex            # GenServer per target (probe scheduling)
      supervisor.ex         # DynamicSupervisor for checkers
      manager.ex            # Target lifecycle (DB → checker start/stop)
      probe.ex              # HTTP probe execution via Finch
      ssrf_guard.ex         # URL validation against SSRF

    errors/                 # Error ingestion subsystem
      ingest.ex             # Pipeline: validate → group → persist → webhook
      grouping.ex           # Template stripping, stack fingerprint, sha256
      rate_limiter.ex       # ETS token bucket (per-key, configurable)
      dedup_cache.ex        # ETS recent group_hash tracking

    alerter/                # Webhook delivery subsystem
      signer.ex             # HMAC-SHA256 signing
      circuit_breaker.ex    # Per-subscription failure tracking
      cooldown.ex           # Per-group flood suppression

    workers/                # Oban background jobs
      webhook_delivery.ex   # Webhook dispatch with retries
      retention_prune.ex    # Daily data pruning
      tls_scan.ex           # Daily TLS expiry checks

  canary_web/
    router.ex               # All HTTP routes
    plugs/
      auth.ex               # API key authentication
      rate_limit.ex         # Per-key rate limiting
      problem_details.ex    # RFC 9457 error responses
    controllers/
      error_controller.ex   # POST /api/v1/errors
      query_controller.ex   # GET /api/v1/query, /errors/:id
      health_controller.ex  # /health-status, /healthz, /readyz
      target_controller.ex  # Target CRUD
      webhook_controller.ex # Webhook subscription CRUD
      key_controller.ex     # API key management
```

## Key Design Decisions

### GenServer vs Oban

**Stateful cadence-sensitive work → GenServer. Stateless retry-oriented work → Oban.**

| What | Owner | Why |
|------|-------|-----|
| Probe scheduling + execution | GenServer | Stateful, cadence-sensitive, needs in-memory state |
| State machine transitions | GenServer | Requires consecutive failure/success counters |
| Webhook delivery + retries | Oban | Stateless, retry-oriented, exponential backoff |
| Retention pruning | Oban | Periodic, no state needed |
| TLS expiry scan | Oban | Daily scan, no persistent state |

### Write Serialization

SQLite is single-writer. `Canary.Repo` has `pool_size: 1` for all writes. `Canary.ReadRepo` has `pool_size: 4` for query API reads. This is explicit and simple — no implicit locking, no concurrent write contention.

### Error Grouping

Three strategies in priority order:
1. **Client fingerprint** — explicit `fingerprint` array in ingest payload
2. **Stack trace fingerprint** — top 5 in-project frames, line numbers stripped
3. **Message template** — regex normalization (UUIDs, timestamps, emails, paths, hex, integers)

Group hash: `sha256(service || discriminator)`. Deterministic, no probabilistic behavior.

### Summary Generation

All query responses include a `summary` string. Generated from deterministic templates — pure functions, no LLM calls, no external dependencies. Tested with unit tests.

## Data Flow

### Error Ingest
```
POST /api/v1/errors
  → Auth plug (API key verification)
  → Rate limit plug (token bucket)
  → ErrorController.create
    → Ingest.ingest (validate → group_hash → persist → webhook)
      → INSERT errors
      → UPSERT error_groups
      → Oban.insert(WebhookDelivery) if new_class/regression
  → 201 {id, group_hash, is_new_class}
```

### Health Check Probe
```
GenServer timer fires
  → SSRFGuard.validate_url
  → Probe.check (Req.get via Canary.Finch)
  → INSERT target_checks
  → StateMachine.transition (pure function)
  → UPDATE target_state
  → Oban.insert(WebhookDelivery) if state changed
  → Schedule next check (interval + jitter)
```

## Persistence

Single SQLite file at `/data/canary.db` with WAL mode. Litestream continuously replicates to Fly Tigris object storage. Recovery is restore-based, not failover.

Tables: `errors`, `error_groups`, `targets`, `target_checks`, `target_state`, `webhooks`, `api_keys`, `seed_runs`, plus Ecto and Oban internal tables.

Retention: 30 days for errors, 7 days for checks, indefinite for everything else.
