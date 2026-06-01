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
35. `canary-server::target_probes::probe_request_tls_expiry`: successful HTTPS
    target probes now capture leaf-certificate expiry in Rust without reopening
    the SSRF hole in Phoenix's raw `:ssl.connect` helper. The capture runs only
    after the HTTP transport returns successfully and after probe latency has
    been measured, then opens a bounded rustls handshake against the same
    `SocketAddr` list produced by target URL validation and DNS SSRF approval.
    Certificate verification is deliberately bypassed for metadata parity with
    Phoenix, but DNS is not repeated and no unapproved address can be dialed.
    Parsing failures, handshake failures, HTTP targets, malformed certificates,
    and non-TLS responses all degrade to `None`; the probe result remains
    governed by the HTTP observation. `TargetProbeOutcome` now carries the
    persisted `tls_expires_at` value so focused runtime tests can prove the
    metadata reached the health observation boundary.
36. `canary-server::TargetProbeLifecycleCommand`: active target probe hot-update
    semantics now have a typed runtime boundary instead of relying on incidental
    next-tick reloads. The lifecycle drains exhaustive target-scoped commands
    before due selection: `Track`, `Untrack`, `Pause`, `Resume`, and
    `Reconfigure`. Runtime pause preserves the schedule but excludes the target
    from due selection, resume pulls the next due time forward to the current
    pass, reconfigure can shorten an interval without pushing an already-due
    probe away, and untrack/pause forget in-memory flap history so stale
    transition windows do not leak across operator control actions. Store also
    exposes the narrow `update_target_active` command needed by future admin
    routes and tests now prove that a target deactivated after the HTTP socket
    opens but before commit is skipped rather than writing a stale observation.
    This keeps hot-update behavior in the target lifecycle module; it does not
    introduce per-target worker threads, cancellation tokens, SQLite update
    hooks, or a generic scheduler.
37. `canary-server::HealthEventFanout`: health-transition webhook enqueue is no
    longer hand-coded as ignored `EventSink` results at each source. Target
    probes, monitor overdue evaluation, and monitor check-ins now dispatch
    committed transition and incident events through one typed fanout boundary
    that returns an `EventFanoutReport` and records advisory enqueue failures by
    source and event. The HTTP response contract stays unchanged: enqueue
    failure after a committed health transition remains best-effort and cannot
    turn a successful check-in into an error. The failure counter is process
    local and intentionally outside SQLite/webhook delivery ledger semantics,
    because delivery ids are created by the enqueue sink and may not exist when
    enqueue itself fails. This gives agents a compile-time boundary to reuse
    instead of reintroducing `let _ = enqueue_event(...)` fanout paths.
38. Rust target admin routes now drive the same typed lifecycle boundary as the
    target probe worker. `GET/POST /api/v1/targets`, `DELETE
    /api/v1/targets/:id`, and `POST /api/v1/targets/:id/{pause,resume}` enforce
    admin scope in Axum, persist through named `canary-store` target commands,
    and best-effort emit `TargetProbeLifecycleCommand::{Track,Untrack,Pause,Resume}` through
    a cloneable `TargetProbeLifecycleController`. The SQLite row remains the
    source of truth because each lifecycle pass reconciles active schedules from
    storage. Create uses the probe module's
    URL, method, header, DNS, and SSRF validation before writing. Pause/resume
    update the `targets.active` flag and existing `target_state` row in one
    store operation, preserving Phoenix's public response while avoiding a
    separate admin scheduler, SQL triggers, or route-local lifecycle state.
39. `PATCH /api/v1/targets/:id` intentionally starts with the only runtime
    target update that changes probe scheduling: `interval_ms`. The store
    returns the prior interval and active flag in a typed
    `TargetIntervalUpdate` outcome, so the Axum route can emit
    `TargetProbeLifecycleCommand::Reconfigure` only after the SQLite update
    commits, only for active targets, and only when cadence changed. Unsupported
    fields fail validation instead of silently expanding the admin contract.
    This keeps target updates agent-friendly and compile-time visible without
    adding a generic CRUD patch layer, state resets, or scheduler-local truth.
40. Rust monitor-overdue parity now has fixtures for the edge cases that made
    the Phoenix behavior easy to damage accidentally. The pure planner proves
    TTL monitors still escalate from degraded to down on `expected_every_ms`,
    not the last check-in TTL. The store proves a failed transition insert rolls
    back the state update, sequence bump, `first_missed_at`, and incident
    correlation because they live inside one SQLite transaction. The server
    adapter treats unsupported persisted monitor enum values as no-op candidate
    rows for overdue evaluation instead of turning one malformed row into a
    failed lifecycle pass. This keeps the Rust rewrite stricter where types own
    behavior, but tolerant at the persisted-data boundary that Phoenix already
    treated as best-effort.
41. `bin/regenerate-phoenix-fixture` now emits both an empty Phoenix-migrated
    schema fixture and a populated Phoenix/Ecto read-model fixture. The
    populated fixture is seeded through Phoenix schemas and changesets with
    production-shaped errors, error groups, target state, monitor state,
    incident signals, annotations, and timeline events. Rust opens that
    Phoenix-created SQLite file directly and proves `Store::error_detail` and
    `Store::incident_detail` can read the joined graph: error-group metadata,
    incident backreferences, incident annotations, recent timeline events,
    target health signals, monitor health signals, and per-subject annotation
    counts. This is intentionally not a claim about now-relative windows or
    list pagination; those need deterministic-clock coverage instead of static
    future timestamps.
42. `canary-store` now exposes deterministic `*_at` query entry points for the
    now-relative read models while leaving the production methods as thin
    `now_utc()` adapters. `errors_by_service_at`, `errors_by_error_class_at`,
    `errors_by_class_at`, and `active_incidents_at` thread a caller-supplied
    `OffsetDateTime` into the same cutoff and 300-second incident activity
    logic used by the default methods. The populated Phoenix fixture now proves
    Rust can evaluate service, error-class, aggregate-class, and active-incident
    read models against Phoenix-created rows at a fixed clock, including the
    empty-after-window summary. This is a store-level test seam only: no HTTP
    `at` parameter, no process-wide clock service, and no change to
    clock-independent detail read models.
43. The populated Phoenix read-model fixture now locks the next edge cases for
    those deterministic store reads. Rust pages more than fifty Phoenix-shaped
    `ramp-api` error groups with the same total-count/group-hash cursor order,
    proves the incident active window is inclusive at exactly 300 seconds,
    distinguishes the error-group activity clock from severity's signal
    attachment clock, and confirms that health signals continue to hold the
    incident open after the error group ages out. The same fixture also drives
    existing health read-model boundaries (`list_targets`,
    `active_target_probe_schedules`, `target_probe_snapshot_by_id`,
    `monitor_check_in_snapshot_by_name`, and `monitor_overdue_candidates`)
    without adding a new health query abstraction.
44. Health-state parsing and active-incident semantics now live on
    `canary-core::health::state_machine::HealthState` instead of being
    re-derived by each Rust caller. `parse_persisted`, `as_str`,
    `incident_signal_active`, and `persisted_incident_signal_active` encode the
    Phoenix-compatible rule that `up` is the only inactive persisted health
    state for health-transition incident signals; `unknown`, `degraded`,
    `down`, `paused`, `flapping`, and even a loaded future non-`up` state keep
    the signal active. Store query reads and incident correlation both call this
    typed rule, while target-probe, monitor-overdue, and check-in parsing use
    the same parser instead of stringly duplicated match tables.
45. Error-group query cursors now document and test their real Phoenix/Rust
    semantics under concurrent ingest. `GroupCursor` is a keyset continuation
    anchor over `(total_count DESC, group_hash ASC)`, not a snapshot token. If a
    later group receives ingest and moves above the saved anchor between page
    requests, the next page does not replay it; agents that need a fresh
    high-frequency view restart from the first page. The store test
    `errors_by_service_cursor_is_a_keyset_anchor_not_a_snapshot` locks this
    behavior with a real `commit_error_ingest` mutation between page requests
    so the rewrite does not accidentally promise snapshot pagination without a
    product/API migration.
46. Incident read-model severity now treats active health-transition signals as
    stateful, not eventful. Error-group signals still use the 300-second
    activity window because their current activity is represented by
    `error_groups.last_seen_at`; health-transition signals have already passed a
    current-state active check, so their `attached_at` age cannot make a still
    non-`up` target or monitor severity-irrelevant. Query-time severity and
    correlation-time stored severity use the same rule so agents do not see a
    `medium` incident while three health checks are still actively failing.
47. Rust now implements the Phoenix admin monitor surface:
    `GET /api/v1/monitors`, `POST /api/v1/monitors`, and
    `DELETE /api/v1/monitors/{id}`. The slice is intentionally hand-shaped like
    the target admin routes, not generalized into a CRUD layer. The store owns
    monitor list/create/delete, server handlers own Axum/auth/body conversion,
    and creation bootstraps the Phoenix-compatible `monitor_state` row with
    `unknown` state. The HTTP parser preserves the OpenAPI/Phoenix contract:
    service defaults to name, mode is `schedule` or `ttl`,
    `expected_every_ms` is required and positive, `grace_ms` defaults to zero
    and may be zero, duplicate names return 422 validation errors, and missing
    deletes return the RFC 9457 404 problem body.
48. Rust now implements the Phoenix admin webhook subscription surface:
    `GET /api/v1/webhooks`, `POST /api/v1/webhooks`,
    `DELETE /api/v1/webhooks/{id}`, and `POST /api/v1/webhooks/{id}/test`.
    The server handler validates only the product contract Phoenix currently
    enforces on creation: non-empty URL and event array, accepted event names
    from the shared business/diagnostic event list, a generated `WHK-` id, a
    one-time 32-character secret in the create response, and no secret exposure
    in list responses. The store owns subscription list/insert/delete over the
    existing webhook table used by the delivery worker, preserving the
    single-writer boundary. The test route reuses the worker request builder and
    outbound transport trait, but calls the blocking HTTP transport through
    `spawn_blocking` and does not create service events, retry jobs, or delivery
    ledger rows.
49. Rust now implements the Phoenix admin API-key surface:
    `GET /api/v1/keys`, `POST /api/v1/keys`, and
    `POST /api/v1/keys/{id}/revoke`. Store owns key metadata listing,
    bcrypt-backed insert rows, active verification, and revocation updates over
    the existing `api_keys` table. Server owns only admin auth, optional JSON
    body decoding, Phoenix defaults (`name = "unnamed"`, `scope = "admin"`),
    one-time raw `sk_live_` key generation, and response conversion. List
    responses expose only metadata plus `active`; create responses are the only
    place the raw key appears. Bcrypt hashing runs through `spawn_blocking` so
    the Axum request task does not perform CPU-heavy password hashing inline.
50. Rust now implements `POST /api/v1/service-onboarding` as the product
    transaction Phoenix exposes to agents. The endpoint validates the
    onboarding request, creates a health target and a scoped ingest-only API
    key, returns copy-paste snippets, and tracks the target after the SQLite
    commit. The server deliberately does not call the existing target or key
    HTTP handlers internally; it builds the typed target/key rows once and asks
    the store to insert both in a single transaction. That keeps the
    agent-facing route simple while preserving the Phoenix contract: trimmed
    service/environment fields, production as the default environment,
    optional positive interval, SSRF-aware URL validation, duplicate service
    and URL validation errors, one-time raw key disclosure, and no lifecycle
    command when auth or validation fails.
51. Rust now implements the Phoenix read-only status surfaces:
    `GET /api/v1/status` and `GET /api/v1/health-status`. The store owns the
    health target/monitor read models and the active error summary; the router
    owns only read-scope auth, query-window defaults, and OpenAPI response
    projection. Target recent checks are fetched with a bounded window-function
    query (`ROW_NUMBER() <= 5`) instead of loading unbounded history, matching
    the Phoenix read-model footgun guard. `/health-status` returns the full
    health target and monitor shapes, while `/status` deliberately trims those
    rows and adds Phoenix-compatible `overall`, deterministic summary text, and
    `error_summary`. Invalid windows reuse the shared RFC 9457 validation
    problem constants.
52. Rust now implements the Phoenix target check history surface:
    `GET /api/v1/targets/:id/checks`. The store owns the bounded history query:
    parse the canonical query window, filter by target id and cutoff, order
    newest first, and cap the result at 500 rows. The server owns only
    read-scope auth, the `24h` default, response projection, and the one
    endpoint-specific Phoenix quirk: invalid windows return the terse
    `"Invalid window."` RFC 9457 detail without the richer `errors.window`
    payload used by `/api/v1/status`. Missing targets intentionally return
    `200` with an empty `checks` array because the Phoenix query reads check
    history directly and does not perform a target existence lookup.
53. Rust now implements the Phoenix timeline replay surface:
    `GET /api/v1/timeline`. The core crate owns the wire DTOs, deterministic
    summary templates, business-event filter set, and Phoenix-compatible
    base64url cursor shape. The store owns the bounded keyset query over
    `service_events`: window cutoff, optional service filter, optional
    comma-separated business event filter, `after`/`cursor` anchor, newest
    first order by `(created_at, id)`, and `limit + 1` pagination capped at
    200. The server owns only read-scope auth, the `24h` default, `after`
    precedence over `cursor`, and RFC 9457 projection for invalid window,
    limit, cursor, and event type inputs. Diagnostic events such as
    `canary.ping` remain valid webhook subscription events but are rejected by
    timeline filtering because Phoenix exposes only business-event replay
    filters there.
54. Rust now implements the Phoenix webhook delivery ledger page:
    `GET /api/v1/webhook-deliveries`. The core crate owns the response DTO,
    explicit nullable fields, delivery cursor shape, and 50/200 page-size
    constants. The store owns the keyset query over `webhook_deliveries`:
    optional webhook id, event, and status filters; newest-first
    `(created_at DESC, delivery_id DESC)` ordering; same-timestamp tie-breaker
    continuity; `limit + 1` pagination; cursor decoding; and the
    Phoenix-compatible `completed_at` derivation for terminal statuses. The
    server owns only read-scope auth, `after` precedence over `cursor`, array
    query rejection for string filters, and RFC 9457 projection for invalid
    limit, cursor, or status inputs. This keeps delivery diagnostics available
    to agents without moving webhook delivery or retry policy into the HTTP
    layer.
55. Rust now implements the Phoenix annotation coordination surface:
    `GET/POST /api/v1/annotations`,
    `GET/POST /api/v1/incidents/:incident_id/annotations`, and
    `GET/POST /api/v1/groups/:group_hash/annotations`. The core crate owns
    the public annotation DTO, legacy list envelope, unified page envelope,
    cursor codec, 50-row limit constants, subject-type set, and exact Phoenix
    summary template including subject-id truncation and latest timestamp. The
    store owns subject existence checks across incidents, error groups,
    targets, and monitors; metadata storage/decoding; legacy `incident_id` and
    `group_hash` backfill; newest-first keyset pagination; total-count summary
    inputs; and typed validation errors. The server owns read/admin scope
    separation, legacy path synthesis, unified subject validation, RFC 9457
    projection, and best-effort `annotation.added` webhook fanout through the
    existing enqueue effect boundary. Annotation actions and metadata stay
    opaque by design; Canary stores coordination facts and wakes consumers, but
    does not interpret responder policy.
56. Rust now implements the Phoenix unified agent report surface:
    `GET /api/v1/report`. The core crate owns the Phoenix-compatible
    offset cursor: base64url JSON carrying independent target, monitor, and
    error-group offsets where `null` means that section is exhausted. The store
    owns the report read models that were missing from earlier slices:
    active window-wide error groups, recent target/monitor transitions, and
    FTS error search with the same quoted-query and BM25 weighting as Phoenix.
    The server owns read-scope auth, query-shape validation, positive-integer
    limit parsing, CSV content negotiation, section pagination, RFC 9457
    projection for invalid window/limit/cursor/query inputs, and the final
    JSON/CSV wire shape. The route remains read-only: no ingest, correlation,
    annotations, or webhook fanout occurs while rendering a report.
57. Rust now implements the Phoenix Prometheus scrape surface: `GET /metrics`.
    The route intentionally stays an admin-scoped snapshot endpoint rather than
    growing a telemetry framework inside the server. `canary-store` owns the
    narrow SQLite reads for durable counters, queue depths, and target/monitor
    state gauges. `canary-core::metrics` owns the Prometheus text exposition
    names, HELP/TYPE headers, and label escaping. The Axum handler owns only
    admin auth, store locking, RFC 9457 failure projection, and the Phoenix
    content type `text/plain; version=0.0.4; charset=utf-8`. Runtime-only
    counters that Phoenix collects from BEAM telemetry remain a future adapter
    concern; this slice exposes the durable operational facts agents can rely
    on during the Rust rewrite.
58. Rust now owns the retention-prune policy and store command that Phoenix
    currently runs through `Canary.Workers.RetentionPrune`. `canary-workers`
    converts a typed retention policy and one observed clock value into the two
    Phoenix cutoffs: errors/service-events use `error_retention_days` and
    target checks use `check_retention_days`. `canary-store` owns the bounded
    rowid-delete loop over `errors`, `service_events`, and `target_checks`,
    returning a typed deletion report. Each 1,000-row delete runs as its own
    SQLite statement, matching Phoenix's `Repo.query!` loop and limiting the
    single-writer lock held by one maintenance batch.
    The Rust implementation keeps the table/column set fixed in code rather
    than accepting dynamic table names from callers. Runtime scheduling and
    operator logging are intentionally left for a later server boot-wiring
    slice; the deletion semantics and tests are now in Rust.
59. Rust now enforces Phoenix-compatible API-key rate limits on the request
    path. `canary-http::rate_limit` owns the named buckets, exact Phoenix
    constants, and RFC 9457 `rate_limited` body with `retry_after`. The server
    owns only process-local fixed-window counters keyed by `(bucket, api_key_id)`
    and calls the limiter immediately after successful scope authorization:
    ingest routes use the 100/minute ingest bucket; read routes, `GET /metrics`,
    and admin annotation writes use the 30/minute query bucket. Public routes
    remain outside auth and rate limiting, and ordinary admin mutations remain
    unrate-limited to match the Phoenix router.
60. `CanaryServer::boot` now starts a named Rust retention-prune lifecycle
    worker. `ServerConfig` owns the explicit cadence and `RetentionPolicy`;
    the default policy stays Phoenix-compatible at 30 days for errors/service
    events and 7 days for target checks. The server worker observes one UTC
    clock per pass, asks `canary-workers` for fixed cutoffs, and calls a new
    `canary-store` one-batch command for each table until old rows are gone.
    Each store mutex guard covers only one 1,000-row delete statement, so a
    long maintenance pass does not monopolize the single SQLite writer across
    the whole multi-batch prune.
61. Rust now accounts for Phoenix's silent `auth_fail` bucket on invalid
    supplied API keys. Missing `Authorization` headers still return the same
    401 Problem Details without touching the bucket; invalid or revoked bearer
    keys increment `RateLimitKind::AuthFail` and deliberately discard the
    limiter result, so even an exhausted invalid-key bucket remains an
    `invalid_api_key` 401 rather than a visible 429. `ServerConfig` exposes an
    explicit `AuthFailIdentityConfig`: proxy-set client IP headers such as
    `fly-client-ip`, `Forwarded`, and `x-forwarded-for` are trusted only when
    enabled, multi-hop proxy headers use the proxy-side/rightmost value rather
    than the client-supplied leftmost value, and the default ignores those
    spoofable headers. The fixed-window limiter now also prunes expired buckets
    during checks so high-cardinality invalid-key traffic does not retain
    stale identities indefinitely. This is a narrow accounting boundary, not a
    generic request-identity framework.
62. Rust now implements the Phoenix TLS-expiry scan as a persisted-data worker,
    not a network probe. Target probing remains the only path that opens HTTP
    or TLS sockets and persists `target_checks.tls_expires_at`; the new
    `canary-workers::tls_scan` planner reads only that timestamp and emits a
    warning for active HTTPS targets whose latest non-null TLS expiry is inside
    the `[0, 14)` day window. `canary-store` owns the active HTTPS/latest-check
    query and records the `health_check.tls_expiring` service event with
    warning severity and Phoenix-compatible payload fields. `canary-server`
    owns the named lifecycle thread, configurable daily cadence, and
    best-effort post-commit webhook enqueue. Expired certificates, including
    partial-day expired timestamps that integer whole-day math could otherwise
    round to zero, do not emit expiring warnings; this is a deliberate
    correctness guard before cutover.
63. The intentional incident-severity divergence is now documented and covered
    against the Phoenix read-model fixture. Phoenix computes incident list
    severity from active signal `attached_at` recency, so three still-failing
    health-transition signals can age from `high` to `medium` after five
    minutes even while their target or monitor state remains non-`up`. Rust
    keeps error-group signals recency-based but treats health-transition
    signals as stateful once the active-signal lookup has proved the target or
    monitor is still failing. The fixture test
    `rust_fixed_clock_queries_keep_phoenix_pagination_and_incident_boundary`
    now locks both sides of the cutover note: two stale active health signals
    stay `medium`, while three stale active health signals remain `high`. This
    is a correctness improvement for agents reading incidents, not an
    accidental silent wire drift.
64. `TargetProbeLifecycle` no longer lets one slow target transport serialize
    every other due target behind it. A lifecycle pass still performs schedule
    reconciliation and command handling on one worker thread, and all writes
    still go through the shared `Store` mutex, but due probes now execute in
    bounded per-pass batches capped by `MAX_CONCURRENT_TARGET_PROBES`. The
    server tests `lifecycle_isolates_fast_due_probe_from_slow_due_probe` and
    `lifecycle_caps_concurrent_due_probe_fanout` lock the two important
    properties: a fast target commits while an unrelated slow target is still
    blocked, and fanout cannot silently become unbounded thread creation.
65. `TargetProbeLifecycle` now reports target probes asynchronously across
    lifecycle ticks. A tick drains completed probe results, launches new due
    probes up to the global `MAX_CONCURRENT_TARGET_PROBES` cap, and returns
    without waiting for slow targets. The lifecycle owns explicit `in_flight`
    state so one target cannot be launched twice while a previous probe is
    still running, and completion-driven schedule advancement happens only
    after the probe thread reports back. Shutdown still exits the coordinator
    without waiting on blocked transports; detached probe threads may finish and
    commit through the same store mutex, but no worker tick is held hostage to a
    hung socket. The report counters now distinguish launches from completions
    (`launched`, `completed`, `in_flight`, and `dropped_untracked`), which makes
    the async contract visible to tests and future agents. The tests
    `lifecycle_reports_completion_on_subsequent_tick`,
    `lifecycle_discards_completion_for_untracked_target`,
    `lifecycle_worker_does_not_duplicate_long_running_probe_before_completion`,
    `lifecycle_worker_shutdown_does_not_wait_for_blocked_probe_transport`, and
    `lifecycle_caps_concurrent_due_probe_fanout` lock the production invariants:
    no duplicate in-flight target probes, bounded global fanout, ignored stale
    completions for removed targets, and bounded worker shutdown.
66. The monitor-overdue and TLS-expiry lifecycle adapters now share the same
    explicit stop-between-work-items contract as the stronger background
    workers without introducing a generic worker framework. `run_due` remains
    the simple public one-pass API, while private `run_due_until` variants let
    the OS-thread workers observe shutdown between persisted candidates and
    report `interrupted` when a pass exits early. Monitor overdue also now
    records lifecycle failures through the worker control path instead of
    silently swallowing store errors or panics. The focused tests
    `monitor_overdue::tests::lifecycle_stops_between_candidates_when_shutdown_is_requested`,
    `monitor_overdue::tests::worker_records_lifecycle_failures`, and
    `tls_scan::tests::lifecycle_stops_between_candidates_when_shutdown_is_requested`
    lock the production invariants: shutdown does not wait for a full backlog
    of sequential candidates, advisory fanout failures remain non-fatal, and
    repeated monitor-overdue lifecycle failures are visible to operators.
67. The Rust server boot path now wires real process-local webhook flood
    controls instead of leaving the typed cooldown and circuit-breaker
    boundaries as no-ops. `InMemoryWebhookCooldown` preserves the Phoenix
    five-minute suppression contract for duplicate event identities after a
    scheduler accepts a job, and `InMemoryWebhookCircuit` preserves the Phoenix
    ten-consecutive-failure threshold plus five-minute probe interval for
    failing subscriptions. Both adapters remain explicit runtime state in
    `canary-server`; the pure delivery planner still receives only
    `CircuitDecision` and cooldown predicates. The focused tests
    `webhooks::tests::in_memory_cooldown_suppresses_until_ttl_expires`,
    `webhooks::tests::in_memory_circuit_opens_probes_and_resets_on_success`,
    and
    `webhooks::tests::in_memory_circuit_failed_probe_reopens_for_another_probe_interval`
    lock the production invariant that Rust webhook delivery cannot flood
    responders in exception loops or hammer an endpoint whose circuit is open.
68. RFC 9457 Problem Details construction now belongs to `canary-http` instead
    of the Axum server. The server still chooses route-specific detail strings
    and domain inputs, but shared wire decisions live beside the
    `ProblemDetails` type: status codes, stable `code` strings, problem type
    slugs, `request_id` nullability, and the canonical `errors` object shape.
    This is intentionally not an `IntoProblem` trait or a server-error enum;
    callers pass the few facts that vary, and the contract crate owns the JSON
    shape. Store-specific knowledge such as `TargetConflict` remains in
    `canary-server`, which converts it to a plain validation error map before
    crossing into the HTTP contract. The focused tests
    `problem_details::tests::validation_factories_preserve_phoenix_details_and_errors`,
    `problem_details::tests::query_and_annotation_problem_factories_preserve_wire_shape`,
    and `problem_details::tests::operational_problem_factories_preserve_status_codes`
    lock the response bodies that agents and SDKs depend on.
69. Public routes and annotation routes are now separate Axum adapters under
    `canary-server::public_routes` and `canary-server::annotations`. This keeps
    `canary-server/src/lib.rs` as the process and route-registration boundary
    instead of the owner of every handler. Public route bodies still come from
    `canary-http::public`; annotation routing still uses the shared server auth,
    rate-limit, response, and post-commit effect boundaries. The annotation
    module owns the subject-specific path adapters, unified annotation query,
    validation parsing, and best-effort `annotation.added` webhook enqueue
    translation. Route strings remain in `ingest_router`, so path drift is easy
    to review. The focused tests
    `healthz_adapts_the_public_contract`,
    `readyz_returns_ready_when_all_dependencies_are_ok`,
    `readyz_returns_503_when_any_dependency_fails`,
    `openapi_serves_the_checked_in_document_unchanged`,
    `public_router_does_not_mount_private_routes`,
    `annotations_create_list_paginate_and_emit_webhook_effect`, and
    `legacy_annotation_routes_and_errors_follow_phoenix_contract` lock the
    unauthenticated and annotation wire contracts after the split.
70. Admin target mutation routes now live in `canary-server::admin_targets`.
    The module owns `GET /api/v1/targets`, target creation, deletion, interval
    patching, pause, resume, target-specific request parsing, target response
    bodies, and the typed target-probe lifecycle commands emitted after
    successful store writes. `ingest_router` still owns the route strings, and
    service onboarding stays in `lib.rs` because it spans target creation plus
    API-key creation in one transaction. Target check history also stays out of
    the admin-target module because it is a read/query surface with different
    scope behavior. The focused tests `admin_target_mutations_emit_lifecycle_commands`,
    `admin_target_interval_update_reconfigures_only_when_cadence_changes`,
    `admin_target_interval_update_rejects_invalid_scope_and_shape`,
    `admin_target_create_rejects_ingest_scope_without_writing_or_commanding`,
    `target_checks_accepts_read_scope_and_returns_recent_checks`, and
    `target_checks_keeps_phoenix_error_and_empty_missing_target_behavior` lock
    the lifecycle command, auth, validation, and neighboring read-route
    contracts after the split.
71. Admin webhook routes now live in `canary-server::admin_webhooks`. The
    module owns webhook subscription listing, creation, deletion, the explicit
    admin test-delivery endpoint, webhook-specific response bodies, validation,
    and the blocking transport handoff used by the test endpoint. `ingest_router`
    still owns the route strings, and webhook delivery ledgers stay out of this
    module because they are read/query surfaces with pagination and status
    filters rather than subscription mutation. The focused tests
    `admin_webhook_mutations_follow_phoenix_contract`,
    `admin_webhook_test_delivery_uses_blocking_transport_boundary`, and
    `admin_webhook_create_rejects_invalid_scope_and_events` lock the existing
    mutation, auth, validation, secret visibility, and blocking transport
    contracts after the split. `admin_webhook_routes_reject_non_admin_scopes`
    and `admin_webhook_test_delivery_maps_inactive_and_request_errors` add
    coverage for adjacent authorization and test-delivery failure branches that
    were easy to blur during extraction.
72. Admin monitor routes now live in `canary-server::admin_monitors`. The
    module owns monitor definition listing, creation, deletion,
    monitor-specific response bodies, and validation for the admin configuration
    surface. `ingest_router` still owns the route strings. Monitor check-ins,
    health/status read models, and the overdue runtime stay out of this module
    because they are ingest or worker surfaces rather than admin configuration.
    The focused tests `admin_monitor_mutations_follow_phoenix_contract`,
    `admin_monitor_create_rejects_invalid_scope_and_shape`, and
    `admin_monitor_routes_reject_non_admin_scopes` lock ID prefixes, default
    service/grace behavior, duplicate-name validation, RFC 9457 error bodies,
    delete semantics, and the no-write forbidden-scope case after the split.
73. Admin API-key routes now live in `canary-server::admin_keys`. The module
    owns API-key metadata listing, direct key creation, revocation,
    admin-key-specific request parsing, and the wire response shapes for key
    lifecycle endpoints. `ingest_router` still owns the route strings, and
    service onboarding stays in `lib.rs` because it atomically creates both a
    health target and a scoped ingest key. The shared create-response
    projection stays in `lib.rs` at that onboarding boundary rather than
    coupling onboarding back to the admin-key module. The focused tests
    `admin_api_key_mutations_follow_phoenix_contract`,
    `admin_api_key_create_defaults_and_rejects_invalid_scope`,
    `admin_api_key_routes_reject_non_admin_scopes`, and
    `service_onboarding_creates_target_ingest_key_and_snippets` lock
    metadata-only list responses, raw-key create semantics, revocation,
    forbidden-scope behavior, and the neighboring onboarding transaction after
    the split.
74. Authenticated health read routes now live in
    `canary-server::health_routes`. The module owns
    `GET /api/v1/health-status`, `GET /api/v1/status`, and
    `GET /api/v1/targets/{id}/checks`, including read-scope enforcement,
    health/status response projections, target-check response projection,
    deterministic summary text, overall status derivation, and the
    target-check invalid-window quirk. `ingest_router` still owns the route
    strings. Public liveness/readiness probes stay in `public_routes`, target
    mutation stays in `admin_targets`, probe execution stays in
    `target_probes`, and reporting only reuses the health projection/summary
    helpers because its report body deliberately embeds those health read
    shapes. The focused tests
    `health_status_accepts_read_scope_and_returns_surfaces`,
    `status_defaults_to_empty_without_surfaces_or_errors`,
    `status_combines_error_summary_with_default_window`,
    `status_rejects_invalid_window_and_missing_auth`,
    `health_read_routes_reject_ingest_scope`,
    `target_checks_accepts_read_scope_and_returns_recent_checks`,
    `target_checks_keeps_phoenix_error_and_empty_missing_target_behavior`, and
    `public_router_does_not_mount_private_routes` lock the response bodies,
    read-scope behavior, default windows, missing-target 200 response, and
    public/private route boundary after the split.
75. Authenticated query read routes now live in
    `canary-server::query_routes`. The module owns
    `GET /api/v1/query`, `GET /api/v1/timeline`, `GET /api/v1/incidents`,
    `GET /api/v1/incidents/{id}`, and `GET /api/v1/errors/{id}`, including
    read-scope enforcement, query-kind selection, default windows, timeline
    cursor precedence, incident annotation filters, and not-found Problem
    Details. `ingest_router` still owns the route strings. Report generation
    remains outside this module because CSV rendering and multi-surface cursor
    pagination deserve their own boundary; webhook delivery reads remain
    separate from webhook subscription mutation and worker delivery execution.
    The focused tests `error_query_accepts_read_scope_and_returns_service_groups`,
    `error_query_service_default_window_is_1h`,
    `error_query_accepts_error_class_with_optional_service_filter`,
    `error_query_accepts_group_by_error_class`,
    `error_query_rejects_ingest_scope_and_invalid_params`,
    `timeline_accepts_read_scope_filters_and_paginates`,
    `timeline_rejects_invalid_params_and_wrong_scope`,
    `incidents_accept_read_scope_and_return_empty_summary`,
    `incidents_filters_with_annotation_and_without_annotation_are_applied`,
    `incidents_reject_ingest_scope`,
    `incident_detail_accepts_read_scope_and_reports_missing_incidents`,
    `incident_detail_rejects_ingest_scope`, and
    `error_detail_accepts_read_scope_and_reports_missing_errors` lock the read
    response bodies, filter behavior, default windows, cursor behavior, auth
    failures, validation errors, and not-found contracts after the split.
76. Report generation now lives in `canary-server::report_routes`. The module
    owns `GET /api/v1/report`, including read-scope enforcement, `q` array
    rejection, default `1h` window, invalid-window mapping, report limit/cursor
    validation, store fan-in, independent target/monitor/error-group
    pagination, report body assembly, optional search results, CSV content
    negotiation, and CSV row shaping. `ingest_router` still owns the route
    string. Cursor encoding remains in `canary-core`, Problem Details factories
    remain in `canary-http`, report read models remain in `canary-store`, and
    canonical health projections remain in `health_routes` so report does not
    duplicate health JSON contracts. The focused tests
    `report_accepts_read_scope_searches_paginates_and_renders_csv`,
    `report_defaults_window_to_1h_and_rejects_invalid_window`, and
    `report_paginates_targets_monitors_and_error_groups_independently` lock
    report auth, search, pagination, CSV, invalid parameter, default-window,
    invalid-window, and independent cursor behavior after the split.

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

1. Continue converging the Rust replacement around small, typed contracts:
   split the remaining Axum route helpers in `canary-server/src/lib.rs` by
   route family so ingest and webhook delivery reads can be reviewed as
   independent adapters without changing wire behavior.
