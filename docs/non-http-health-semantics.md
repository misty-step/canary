# Non-HTTP Health Semantics

Decision date: 2026-04-17

## Problem

Canary's current health model is URL-centric: `Target` records point at HTTP
surfaces, `Health.Checker` polls them, and `Status` / `Report` summarize the
resulting `TargetState`.

That works for HTTP services, but it does not cover:

- desktop apps that are not supposed to expose a stable public URL
- cron jobs and batch workers that are only "healthy" if they check in on time
- long-running background workers where silence matters more than a single crash

We need one canonical non-HTTP health model that:

- preserves Canary as the source of truth
- fits existing direct-to-Canary integrations
- reuses the timeline / webhook model agents already consume
- does not require a dashboard-first or third-party relay architecture

## Current Seams

- Canary already has a durable append-only event log in `service_events` and a
  canonical replay API in `/api/v1/timeline`.
- `time-tracker` already reports actual crashes and errors to `POST /api/v1/errors`,
  but it has no native liveness model in Canary.
- `cerberus` currently translates health transitions into `POST /api/v1/errors`,
  which preserves alert noise but loses first-class health semantics.

The design should keep `/api/v1/errors` for errors and add a distinct path for
"this runtime is still alive / this scheduled run completed / this run failed."

## Options

| Approach | Fit | Strengths | Weaknesses | Verdict |
|---|---|---|---|---|
| Heartbeat / check-in monitors | cron jobs, workers, active desktop sessions | Detects silence, supports start/success/failure, direct HTTP from clients, matches Canary's event-log model | Requires a new monitor state store and admin surface | Selected |
| Hosted relay | cron jobs, workers | Cheap for clients, familiar pattern | Splits the source of truth, weakens dogfooding, adds external dependency and secret sprawl | Rejected |
| Local companion process | desktop apps | Can observe local state and expose richer device semantics | Adds packaging, updates, support burden, and another always-on component | Rejected for v1 |
| Crash-only via `/api/v1/errors` | all runtimes | Minimal code, already available | Cannot detect missed runs, hangs, or silent death; overloads error ingest with health semantics | Rejected |

## Reference Patterns

The selected model matches the strongest parts of current production prior art:

- Healthchecks.io models periodic jobs as secret ping URLs with `start`,
  `success`, and `failure` signals plus period and grace-time state transitions:
  https://healthchecks.io/docs/
  https://healthchecks.io/docs/http_api/
- Better Stack heartbeats use the same expected-frequency plus grace-period
  pattern, with explicit failure reporting:
  https://betterstack.com/docs/uptime/cron-and-heartbeat-monitor/
- Sentry treats scheduled work as monitors and check-ins, and desktop / app
  stability as session health rather than HTTP uptime:
  https://docs.sentry.io/api/crons/create-a-monitor/
  https://docs.sentry.io/api/crons/retrieve-checkins-for-a-monitor/
  https://docs.sentry.io/platforms/rust/guides/axum/configuration/releases/
- Electron's native `crashReporter` is useful crash telemetry, but it is only a
  crash transport. It does not answer "is the runtime alive right now?":
  https://www.electronjs.org/docs/latest/api/crash-reporter

Inference from those sources: Canary should merge "cron monitor" and "desktop
session heartbeat" into a single check-in surface, because our product already
has one unified timeline and incident model rather than separate product silos.

## Decision

Adopt **check-in monitors** as Canary's canonical non-HTTP health model.

### Entity Model

Add a new admin-managed entity, separate from `Target`, for non-HTTP runtimes.

- `Target` remains URL-backed and polled.
- `Monitor` (name flexible in implementation) represents a non-HTTP runtime
  that reports its own freshness.
- Do not overload `Target` with fake URLs or local loopback relays.

### Canonical Write Surface

Add one new writer endpoint:

```json
POST /api/v1/check-ins
{
  "monitor": "time-tracker-active-timer",
  "status": "alive",
  "check_in_id": "c0f6f96a-97a6-46b8-b0a5-2b4b1f6a2cf6",
  "observed_at": "2026-04-17T22:30:00Z",
  "ttl_ms": 90000,
  "summary": "active timer still running",
  "context": {
    "platform": "electron",
    "timer_running": true
  }
}
```

Supported statuses:

- `alive`: refreshes a TTL-based monitor's freshness window
- `in_progress`: opens a scheduled run or long job
- `ok`: closes a run successfully
- `error`: closes a run as failed

`check_in_id` is optional but recommended when a client wants Canary to match a
specific `in_progress` signal with a later `ok` or `error`.

### Monitor Modes

Use one API with two server-side monitor modes:

- `schedule`: for cron jobs, report generators, queued workers, replication
  checks, and other runtimes that are expected on a fixed cadence
- `ttl`: for active desktop sessions or long-running workers that need a moving
  freshness window instead of cron semantics

This keeps the client protocol small while still covering the actual runtime
shapes we own.

## Event and Incident Semantics

Do not invent a second replay channel for non-HTTP health.

Instead:

- store monitor state separately from `TargetState`
- emit timeline events through `service_events`
- reuse the existing `health_check.degraded`, `health_check.down`, and
  `health_check.recovered` business events so agent consumers do not need a
  transport-specific subscription split
- differentiate the payload by entity shape, e.g. `"monitor": {...}` instead of
  `"target": {...}`

`POST /api/v1/errors` remains the write path for actual crashes, stack traces,
and warnings. A missed heartbeat is health state, not an error event.

## Runtime Mapping

### Desktop apps

Desktop apps are only monitorable when there is an explicit expectation of
liveness.

For `time-tracker`, that means:

- keep using `POST /api/v1/errors` for crashes and application errors
- use a `ttl` monitor only for active runtime states that should stay fresh
  (for example, an active timer session or a long-running background sync)
- do not alert simply because the app is closed by choice

### Cron jobs and scheduled workers

These should use `schedule` monitors and send:

- optional `in_progress`
- `ok` on successful completion
- `error` on failure

Silence beyond `expected_every + grace` becomes the primary unhealthy signal.

### Cerberus-style health transitions

`cerberus` should stop encoding health transitions as synthetic error events.

Its future path should be:

- health / freshness via `POST /api/v1/check-ins`
- actual detected errors via `POST /api/v1/errors`

## Why Not the Other Options

### Hosted relay

This would reproduce the exact thing Canary is supposed to own: monitor
definitions, last-seen timestamps, grace windows, and incident semantics. We
would dogfood someone else's product instead of our own.

### Local companion

A sidecar or background daemon could make desktop health richer, but it creates
another installable artifact, another update stream, another crash surface, and
another support matrix. That is not the right first move.

### Crash-only

Crash-only telemetry answers "did something explode?" It does not answer:

- did the scheduled task fail to run?
- did the worker hang silently?
- did the desktop app stop heartbeating mid-session?

That is insufficient for Canary's health lane.

## Follow-On

The implementation follow-up is tracked in
[backlog.d/021-check-in-monitors-for-non-http-runtimes.md](/Users/phaedrus/Development/canary/backlog.d/021-check-in-monitors-for-non-http-runtimes.md).
