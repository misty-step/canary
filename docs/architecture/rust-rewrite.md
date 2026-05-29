# Canary Rust Rewrite Architecture

This document is the working architecture map for the full Rust rewrite. It is
not a compatibility waiver: the Rust service must preserve Canary's agent-facing
contracts while moving correctness guards into Rust types, exhaustive enums, and
contract tests.

## Strategic Design Rules

- Deep modules own hard decisions. HTTP handlers route and translate; domain
  crates validate, classify, transition, persist, and emit typed outcomes.
- Wire contracts are stable product contracts. OpenAPI, RFC 9457 Problem
  Details, signed webhook headers, ID prefixes, and scoped API keys must remain
  compatible unless a migration document explicitly breaks them.
- SQLite remains a single-writer store until the product requirement changes.
  Rust must encode the writer boundary explicitly instead of hiding contention
  behind generic pools.
- No semantic wrappers around generic agents. Agent-facing replay, timelines,
  incidents, and summaries are deterministic data products.
- State machines stay pure. Persistence, webhooks, metrics, and logging consume
  typed effects returned by pure modules.

## Crate Layout

```text
crates/
  canary-core/      # typed IDs, health FSM, grouping, classification, incidents
  canary-http/      # RFC 9457, auth/scope wire behavior, OpenAPI parity helpers
  canary-store/     # SQLite schema, migrations, single-writer repository
  canary-ingest/    # validates payloads and commits grouped errors
  canary-events/    # timeline ledger and event fanout
  canary-workers/   # webhook delivery, retention, TLS scan, retry ledger
  canary-server/    # Axum router, app wiring, config, telemetry, shutdown
```

The first server crate now exists, but it is intentionally an adapter, not a
new product layer. `canary-server` mounts public unauthenticated routes whose
bodies are built by `canary-http::public`, plus the authenticated
`POST /api/v1/errors` adapter that performs only HTTP-boundary work:
content-length preflight, bearer/scope checks, JSON decoding, response status
selection, and RFC 9457 translation. Validation, grouping, classification, and
the database commit stay in `canary-ingest` and `canary-store`.

The crate boundaries should stay deep:

- `canary-core` owns pure domain decisions and exposes typed outcomes.
- `canary-http` owns wire translation and compatibility helpers.
- `canary-store` will hide SQLite, migrations, and the single-writer boundary.
- `canary-ingest` will expose one high-level `ingest` operation rather than a
  scatter of validation, grouping, classification, and incident hooks.
- `canary-events` will make timeline append plus webhook fanout one committed
  operation, so callers cannot forget half of the product contract.

Avoid small crates or modules that only rename another layer. In the Phoenix
service, thin facades such as summary/status/report response builders are useful
locally but should not become Rust crate boundaries.

## Current Parity Anchors

- Endpoint map: `priv/openapi/openapi.json` and `lib/canary_web/router.ex`.
- Error body shape: `lib/canary_web/plugs/problem_details.ex`.
- Typed ID prefixes: `lib/canary/id.ex`.
- Pure health transitions: `lib/canary/health/state_machine.ex` and
  `test/canary/health/state_machine_test.exs`.
- Error grouping and classification: `lib/canary/errors/grouping.ex`,
  `lib/canary/errors/classification.ex`, and `lib/canary/errors/ingest.ex`.
- SQLite schema: `priv/repo/migrations/*.exs`.
- Webhook delivery contract: `lib/canary/workers/webhook_delivery.ex`.
- Footguns to encode, not rediscover: `CLAUDE.md`.

## Compatibility Rules

These details are easy for agents to break and should become golden tests before
the Rust server accepts production traffic:

- JSON request body limit remains 102400 bytes. `POST /api/v1/errors` also keeps
  its content-length preflight before JSON parsing.
- Problem Details bodies use `type`, `title`, `status`, `detail`, `code`,
  optional `request_id`, and flattened metadata. The `type` URL remains
  `https://canary.dev/problems/<dash-code>`.
- Authorization accepts exactly `Bearer <key>` after the prefix. Scopes remain
  `ingest-only`, `read-only`, and `admin`, with admin accepted everywhere.
- Rate limit policies remain `ingest: 100/60s`, `query: 30/60s`, and
  `auth_fail: 10/60s`. `retry_after` stays in the Problem Details body.
- Query windows remain the closed enum `1h`, `6h`, `24h`, `7d`, and `30d`.
- Cursor precedence remains `after` before `cursor` where both are accepted.
- Error ingest validation order remains required fields, context-size limit,
  then fingerprint validation.
- Truncation limits remain message 4096 bytes, stack trace 32768 bytes, and
  context 8192 bytes.
- Webhook headers remain `content-type`, `x-signature`, `x-event`,
  `x-delivery-id`, `x-webhook-version`, and `x-sequence`.
- Webhook delivery keeps stable `X-Delivery-Id` across retries, HMAC body
  signing as `sha256=<hex>`, four attempts, and backoff of 1, 5, 30, and 60
  seconds.
- Empty success responses remain HTTP 204 with no JSON body.

## First Implementation Slice

1. `canary-core::ids`: prefixed newtypes for `ERR`, `INC`, `TGT`, `MON`,
   `WHK`, `KEY`, `ANN`, `CHK`, `EVT`, and `DLV`.
2. `canary-core::health::state_machine`: pure transition function with typed
   states, events, thresholds, counters, and effects.
3. `canary-core::ingest::grouping`: grouping priority for client fingerprints,
   stack traces, and normalized message templates.
4. `canary-core::ingest::classification`: deterministic classification rules
   for category, persistence, and component.
5. `canary-http::problem_details`: RFC 9457 body compatible with the Phoenix
   implementation.
6. `canary-http::auth`: bearer-header extraction, scoped API-key authorization
   decisions, and Phoenix-compatible 401/403 Problem Details bodies.
7. `canary-http::public`: public unauthenticated endpoint contracts for
   `/healthz`, `/readyz`, and `/api/v1/openapi.json`, including unchanged
   OpenAPI bytes from `priv/openapi/openapi.json`.
8. `canary-server`: an Axum public-router adapter for `/healthz`, `/readyz`,
   and `/api/v1/openapi.json` that preserves status codes, content type, body
   bytes, and the absence of private routes.
9. `canary-http::webhooks`: HMAC-SHA256 signing, verification, and outbound
   webhook header construction for exact body bytes, including Phoenix parity
   fixtures for `sha256=<hex>`, `x-delivery-id`, `x-event`,
   `x-webhook-version`, and `x-sequence`.
10. `canary-store`: a single-writer SQLite boundary with ordered schema
    migrations ported from the Phoenix Ecto migrations, plus compatibility tests
    for table shape, defaults, indexes, FTS triggers, foreign keys, and
    open-incident uniqueness.
11. `canary-store::commit_error_ingest` and `canary-ingest`: transactional
    error persistence plus a deep ingest boundary that owns Phoenix validation
    order, truncation, grouping, classification, and the single store call.
12. `canary-server::ingest_router`: an Axum adapter for `POST /api/v1/errors`
    that preserves content-length preflight, `admin`/`ingest-only` authorization,
    malformed JSON handling, validation/413 Problem Details, and the 201 ingest
    summary without putting domain decisions in the router.
13. `canary-store::active_incidents` and `GET /api/v1/incidents`: a read-only
    incident list path that preserves scoped read auth, active-signal filtering,
    annotation include/exclude filters, severity derivation, and deterministic
    list summaries.
14. `canary-store::incident_detail` and `GET /api/v1/incidents/:id`: a bounded
    incident detail path that preserves stored incident state, total signal
    counts, newest-first signal and annotation caps, per-signal subject
    annotation counts, recent timeline events, and deterministic action briefs.
15. `canary-ingest::IngestEffect` and `canary-server::IngestEffectSink`: a
    typed post-commit effect boundary for broadcasts, incident correlation, and
    webhook enqueue triggers. The SQLite commit remains the only ingest-critical
    operation; effect sink failures are best-effort and do not change the 201
    ingest summary.
16. `canary-store::webhook_deliveries` and `canary-workers::webhooks`: the
    first webhook delivery port. Store owns idempotent pending/suppressed ledger
    rows, attempt/delivered/discarded transitions, deterministic list filters,
    and active subscription filtering. Workers own Phoenix-compatible delivery
    IDs, retry classification, backoff, header request construction, and
    cooldown identity without importing an Oban-equivalent runtime.
17. `canary-workers::webhooks::plan_enqueue_for_event` and
    `canary-server::WebhookEnqueueEffectSink`: the Rust ingest
    `EnqueueWebhook` effect now reaches the delivery boundary. Workers produce
    explicit schedule-or-suppress decisions, Store persists pending,
    suppressed, and enqueue-failed ledger outcomes, and Server wires those
    decisions through injected scheduler and cooldown traits after the ingest
    commit. Scheduler failures remain best-effort and do not change the 201
    ingest response.
18. `canary-workers::webhooks::execute_delivery` and
    `canary-store::webhook_subscription`: the scheduled delivery contract now
    has a Rust executor that resolves a stable delivery id, emits ordered ledger
    actions, builds the signed HTTP request through injected transport,
    classifies delivered/retry/discard outcomes, returns retry backoff, and
    requests circuit success/failure effects. Store can look up a subscription
    by id including inactive rows, so the executor can distinguish missing,
    inactive, open-circuit, retryable, and final-discard cases. This is still a
    pure/injected execution boundary, not a background job loop or real HTTP
    client.
19. `canary-workers::webhooks::try_execute_delivery` and
    `canary-server::WebhookDeliveryRuntime`: the executor now has a fallible
    ledger-recorder variant so a failed pending or attempt write stops before
    transport. Server owns the one-job runtime adapter that looks up
    subscriptions, asks the circuit boundary, applies `DeliveryLedgerAction`
    values to Store in order, invokes an injected transport, and records circuit
    effects. This proves the delivery side-effect boundary without introducing
    a generic job framework, polling loop, retry table, or concrete HTTP client.
20. `canary-server::webhooks`: webhook enqueue and one-job delivery runtime
    wiring now lives in a focused private server module with root re-exports for
    the public traits and adapters. This keeps the crate API stable while moving
    webhook-specific Store mapping, runtime boundaries, and timestamp helpers
    out of the Axum router surface. The split is intentionally one module, not a
    lifecycle taxonomy or a new crate.
21. `canary-store::oban_jobs`, `canary-server::StoreWebhookScheduler`, and
    `canary-server::WebhookDeliveryDrain`: webhook delivery now has a concrete
    Oban-compatible scheduled-job adapter. Store owns insertion, due-job
    claiming, attempt increments, and the single completion transition for
    retry/complete/discard. Server owns the bounded sequential drain that turns
    claimed rows into `WebhookJob` values, invokes `WebhookDeliveryRuntime`,
    persists retry scheduling with the same delivery id, and exposes an explicit
    max-jobs limit. This remains a bespoke webhook delivery adapter, not a
    generic job framework or alternate scheduler abstraction.
22. `canary-server::HttpWebhookTransport`: outbound webhook delivery now has a
    concrete HTTP transport behind the existing `WebhookTransport` trait. It
    sends the already-signed `WebhookRequest` body bytes unchanged, forwards the
    six Phoenix-compatible webhook headers, maps response status codes directly
    into `TransportResult::HttpStatus`, maps connection failures into
    `TransportResult::RequestError`, disables redirects, and relies on the
    scheduler for all retry/backoff authority. The implementation is blocking
    and must be wired from a dedicated worker/drain context rather than an Axum
    request task.
23. `canary-server::WebhookDeliveryDrainWorker`: scheduled webhook delivery now
    has a concrete lifecycle adapter that runs `WebhookDeliveryDrain` on one
    named OS thread. The worker drains immediately, repeats on an explicit
    interval, exposes stop/join shutdown, and wakes promptly when stopped so
    long idle intervals do not delay process teardown. Each pass is isolated so
    a panic in a transport or drain dependency does not permanently kill webhook
    delivery. This keeps the blocking `HttpWebhookTransport` path outside Axum
    request tasks while preserving the existing bounded drain and avoiding a
    generic scheduler or job framework.
24. `canary-server::CanaryServer`: the Rust service now has a top-level
    bootstrap surface that opens and migrates the configured SQLite database,
    shares the single-writer store across authenticated routes, webhook enqueue,
    and the scheduled delivery drain, exposes one composed Axum router, and
    provides a graceful serve boundary. Blocking webhook transport
    initialization stays on an OS thread so the bootstrap is safe to call from
    async tests without turning Canary into a runtime framework.
25. `canary-store::correlate_incident` and
    `canary-server::RuntimeIngestEffectSink`: the Rust ingest path now turns
    `CorrelateIncident` effects into incident rows, signal attachments, timeline
    service events, and `incident.opened` webhook enqueue requests. Store owns
    the whole correlation transaction: signal activity checks, first-open,
    update, deterministic resolution, severity escalation, and event payload
    construction. Server owns only effect adaptation, generated ids, current
    time, and best-effort webhook enqueue. This keeps correlation behind one
    deep persistence method instead of spreading incident rules through Axum
    handlers or worker glue.
26. `crates/canary-store/tests/phoenix_fixture_compat.rs`: the Rust store now
    has a checked Phoenix-migrated SQLite fixture gate before production
    traffic moves. The fixture preserves Ecto's `schema_migrations` ledger and
    `user_version = 0`; Rust tests compare tables, product columns, explicit
    indexes, partial unique indexes, foreign keys, and the FTS trigger surface
    against a fresh Rust-migrated schema. The same fixture is copied into
    temporary writable databases to prove `Store::migrate` can restamp a
    Phoenix DB without deleting the Ecto ledger and that Rust ingest, incident
    correlation, FTS, and webhook delivery queries work against the
    Phoenix-shaped file. `bin/regenerate-phoenix-fixture` is the only intended
    refresh path, and it uses a partitioned Phoenix test database so normal
    local test state is not the fixture source.
27. `canary-store::commit_health_transition`: target and monitor health
    transitions now enter the store through one deep command boundary. The
    command writes the appropriate health state row, appends the deterministic
    `health_check.*` service event payload, and runs incident correlation in
    the same SQLite transaction, so `health_transition` signal activity is read
    from the state row written by the transition itself. HTTP target and
    non-HTTP monitor payloads remain distinct variants of one health signal
    concept; callers do not sequence `target_state`/`monitor_state`, timeline,
    and incident writes themselves.
28. `canary-store::commit_target_probe` and
    `canary-store::commit_monitor_check_in`: observed health input now has its
    own deep store boundary before runtime wiring. A target probe always inserts
    its `target_checks` row and updates `target_state`; a monitor check-in
    always inserts its `monitor_check_ins` row and updates `monitor_state`.
    Transition metadata is optional. When present, the same transaction also
    appends the deterministic `health_check.*` service event and correlates the
    health signal into incidents; when absent, the observation updates counters,
    timestamps, deadlines, and last-success/failure fields without bumping the
    transition sequence or writing timeline/incident rows. This matches the
    Phoenix distinction between "every probe/check-in is persisted" and "only
    state changes emit transition products."
29. `canary-workers::health`: target probe and monitor check-in runtime
    decisions now have a typed pure planning layer above the store command
    boundary. The planner consumes already-observed target probe results,
    current target snapshots, monitor check-in input, and generated ids, then
    emits the exact `canary-store` commit command to persist. Target probes are
    routed through the pure `canary-core` state machine, including flap
    detection; only `health_check.recovered`, `health_check.degraded`, and
    `health_check.down` webhook effects produce transition metadata. Monitor
    check-ins preserve Phoenix semantics directly: `error` maps to `down`,
    `alive`/`ok`/`in_progress` map to `up`, TTL deadlines use the observed
    timestamp plus positive check-in TTL only in TTL mode, and `in_progress`
    updates liveness without stamping `last_success_at`. The module explicitly
    does not execute HTTP requests, perform SSRF checks, schedule probes, own
    SQLite transactions, or enqueue webhooks; the next runtime adapters must
    supply serialized per-target snapshots or transactionally locked reads.
30. `canary-server::create_check_in` and
    `canary-store::monitor_check_in_snapshot_by_name`: the Rust service now
    accepts non-HTTP monitor check-ins on `POST /api/v1/check-ins` under the
    same ingest-scope auth boundary as Phoenix. The handler reuses the existing
    JSON size/auth/problem-details adapters, loads the monitor configuration and
    current state while holding the single store mutex, feeds that snapshot into
    `canary-workers::health::plan_monitor_check_in`, commits the resulting
    `MonitorCheckInCommit`, returns the committed state sequence from the store
    transaction, and best-effort enqueues the recorded health/incident service
    events after commit. Store owns monitor lookup, missing state bootstrap,
    observation persistence, timeline payload construction, and incident
    correlation; the HTTP layer does not re-derive sequence numbers or state
    transitions. This wires the planner into a real runtime path without adding
    a scheduler, overdue evaluator, or target probe executor.
31. `canary-server::target_probes` and
    `canary-store::target_probe_snapshot_by_id`: the Rust service now has a
    concrete single-target probe adapter instead of a scheduler-shaped
    abstraction. Store owns active target lookup, Phoenix service-name fallback,
    missing `target_state` bootstrap, and current counter snapshots. The server
    adapter owns runtime-only concerns: URL/method/header validation, DNS
    resolution, non-global address blocking, redirect-disabled HTTP execution,
    Phoenix-compatible status/body result mapping, timeout/DNS/TLS/connection
    error classification, bounded response body reads, target probe planning,
    store commit, and post-commit health/incident webhook fanout. The real
    `ReqwestProbeTransport` pins reqwest resolution to the addresses that passed
    the SSRF guard while preserving the original host in the URL for Host/SNI.
    Tests cover blocked probes without opening transport, successful
    probe-to-commit-to-event fanout, response mapping, and non-global IP
    classification. TLS expiry is still a guarded transport extension: the
    current reqwest transport returns `None` rather than opening Phoenix's
    second raw TLS socket path without pinning it to the same SSRF-approved
    address set. This still deliberately avoids periodic scheduling, jitter,
    telemetry, and cross-restart flap history; those belong to the target
    runtime lifecycle slice, not the single-observation adapter.
32. `canary-server::TargetProbeRuntime`,
    `canary-server::TargetProbeLifecycle`, and
    `canary-store::active_target_probe_schedules`: active HTTP target probes now
    have a Rust lifecycle adapter wired into `CanaryServer::boot`. Store owns the
    narrow active-target schedule query. Server owns the rest of the runtime
    boundary: one named lifecycle worker, explicit stop/pause/resume hooks,
    bounded sequential due passes, deterministic interval jitter, active-target
    reconciliation, in-memory per-target transition history for flap detection,
    and panic isolation around each pass. The lifecycle consumes the existing
    single-probe adapter rather than exposing a scheduler/job framework, and
    post-commit event fanout still goes through the same webhook enqueue sink as
    ingest and monitor check-ins. Tests lock active-only loading, no duplicate
    due execution before the jittered interval, zero-interval rejection, and
    runtime transition history driving the pure flapping state machine. TLS
    expiry capture, hot target control semantics, and cross-restart transition
    history remain separate slices.
33. `canary-workers::health::plan_monitor_overdue`,
    `canary-store::commit_monitor_overdue`, and
    `canary-server::MonitorOverdueLifecycle`: non-HTTP monitors now have the
    Phoenix overdue path in Rust without pretending a missed deadline is a
    check-in. Store exposes only deadline-bearing monitor-state candidates and a
    separate overdue transition command that updates `monitor_state`, appends
    the deterministic `health_check.degraded` or `health_check.down` service
    event, and correlates the health signal in one transaction. The worker
    planner owns the parity decision matrix: `now > deadline_at`, last status
    `error` noops, existing `down` noops, `unknown`/`up` become `degraded` with
    `first_missed_at = now`, and `degraded` becomes `down` only after
    `expected_every_ms` has elapsed from the first miss. Malformed persisted
    overdue timestamps noop so one bad row does not abort a lifecycle pass. The
    server adapter is deliberately bespoke and small: one named worker loads
    candidates, calls the planner, commits through the store command, and
    best-effort enqueues the already-recorded transition/incident events. It
    does not introduce a generic scheduler, does not write SQL, and does not
    insert `monitor_check_ins`.
34. `canary-server::target_probes::parse_headers`: configured target headers
    now have the validation Phoenix lacked before the transport opens a socket.
    The persisted shape stays simple and compatible — a JSON object of string
    values — but Rust parses each header name and value with the HTTP header
    types, normalizes names case-insensitively, rejects duplicate normalized
    names, rejects authority/framing/hop-by-hop headers owned by Canary's probe
    transport (`Host`, `Content-Length`, `Transfer-Encoding`, `Connection`,
    `Keep-Alive`, `Upgrade`, `TE`, `Trailer`, `Expect`, and proxy hop headers),
    and bounds both entry count and serialized header bytes. Validation failures
    follow the same runtime shape as SSRF failures: no transport call is opened,
    a `connection_error` target check is still committed, and the existing state
    machine decides whether that failed observation changes health state. This
    keeps header security at the target-probe boundary instead of spreading it
    through store schemas, target lifecycle scheduling, or reqwest transport
    error handling.

This slice is deliberately small but aligned with the full rewrite: it moves
existing contracts into Rust types and tests. The server crate is allowed
to know Axum, routing, and response conversion; it is not allowed to own product
decisions already expressed by `canary-core` or `canary-http`.

## Verification Expectations

Every migration slice needs both Rust-native tests and parity tests against the
Phoenix behavior until the replacement is complete:

- Unit tests cover pure behavior in `canary-core`.
- Golden tests lock wire bodies, headers, IDs, HMAC signatures, and OpenAPI
  responses.
- Property tests cover normalization, parser round trips, ID parsing, and state
  machine invariants.
- Database tests run migrations into a temporary SQLite database and assert both
  schema shape and repository behavior.
- HTTP tests exercise the same endpoint, auth, and error cases in the OpenAPI
  contract.
- The repo gate calls Rust from `./bin/validate`: fast validation runs
  `cargo fmt --all --check` and `cargo check --workspace --all-targets --locked`;
  deterministic validation runs clippy and tests; advisory validation runs
  `cargo audit`.

## Next Slices

1. Harden the target probe runtime edge cases that were deliberately left out of
   the lifecycle slice: guarded TLS expiry capture using the same SSRF-approved
   address set, enqueue-failure telemetry, and explicit hot-update semantics for
   target deactivate/pause/update while a probe is in flight.
2. Broaden monitor overdue parity fixtures around malformed persisted rows,
   TTL-vs-expected escalation, webhook enqueue failure receipts, and transaction
   rollback evidence for transition/correlation failures.
3. Add a populated Phoenix fixture once health and annotation writes are ported
   so Rust read models are checked against Phoenix-inserted production-shaped
   rows, not only an empty migrated schema.
