---
name: monitor
description: |
  Post-deploy signal watch. Poll healthcheck and configured signals through
  a grace window. Emit structured events. Escalate to /diagnose on trip,
  close clean on green. Thin watcher, not diagnostician.
  Use when: "monitor signals", "watch the deploy", "is the deploy ok",
  "post-deploy watch", "signal watch", "grace window", "watch production".
  Trigger: /monitor.
argument-hint: "[<deploy-receipt-ref>] [--grace <duration>] [--config <path>]"
---

# /monitor

This skill is the harness-side post-deploy signal watcher — not Canary's
product-monitor concept (non-HTTP check-ins watched by `Canary.Monitors`
under `docs/non-http-health-semantics.md`). Do not conflate.

Watch the `canary-obs` Fly app after `/deploy` pushes it. Escalate to
`/diagnose` on regression. Close clean when signals stay green through
the grace window.

This skill observes and escalates. It does not diagnose root cause
(`/diagnose` does). It does not rollback (operator decides — see the
"Nuclear reset" block in `CLAUDE.md`). It does not page humans
(`.github/workflows/uptime-monitor.yml` already files GitHub issues on
a slower cadence; do not duplicate).

## Execution Stance

You are a thin watcher.
- Poll `/healthz` + `/readyz` + self-ingest on a fixed cadence until the
  grace window elapses or a signal trips.
- On trip: emit one `deploy.monitor.tripped` event with the signal name,
  endpoint, and a small slab of recent self-ingested errors, then hand
  off to `/diagnose`.
- On clean: emit `deploy.monitor.closed { healthy }` and stop.
- Never analyze *why* a signal tripped. Never attempt remediation (no
  `flyctl machines restart`, no DB wipes, no webhook resets).

The product-monitor surface (`GET/POST /api/v1/monitors`, `POST
/api/v1/check-ins`, modes `schedule` / `ttl`, `expected_every_ms`) is
out of scope for this skill. If the user is asking about desktop
heartbeats or cron check-ins, redirect to `docs/non-http-health-semantics.md`.

## Inputs

| Input | Source | Default |
|-------|--------|---------|
| deploy receipt ref | positional arg from `/deploy` | required in outer loop; absent → healthcheck-only on `https://canary-obs.fly.dev` |
| grace window | `--grace` flag, else built-in | 5 minutes from deploy receipt |
| poll interval | built-in | 15 seconds |
| read key | `$CANARY_READ_KEY` (scope `read-only`) | required for `/api/v1/*`; `/healthz` + `/readyz` are public |

The `.github/workflows/deploy.yml` workflow fires on `workflow_run`
after `ci.yml` completes green on `master`. `/monitor` runs *after*
that deploy succeeds — against live `canary-obs`.

## Signals

All signal URLs target `https://canary-obs.fly.dev` unless noted. The
first two are unauthenticated; everything under `/api/v1/*` requires
`Authorization: Bearer $CANARY_READ_KEY`.

1. **Liveness — `GET /healthz`.** HTTP router alive. Non-200 is a
   **hard trip** (one-shot). Matches what
   `.github/workflows/uptime-monitor.yml` polls every 5 minutes; this
   skill polls faster through the grace window.
2. **Readiness — `GET /readyz`.** DB reachable + supervisor tree
   healthy (`Canary.Repo` pool_size:1, SQLite single-writer). Non-200
   is a **hard trip**. A 200 from `/healthz` paired with non-200 from
   `/readyz` is the classic "router up, DB migration/boot still
   racing" failure — the `Health.Manager` boot `rescue` path retries
   every 5s, so give it 15–30s before concluding it is truly stuck.
3. **Fly machine status — `flyctl status --app canary-obs`.** All
   machines `started`, no restarts within the grace window. A machine
   cycling `started → stopping → started` mid-window is a hard trip
   even if `/healthz` happens to answer between restarts.
4. **Self-ingested errors — `GET /api/v1/query?service=canary&window=5m`.**
   Canary reports its own errors via `Canary.ErrorReporter` — a direct
   `:logger` handler that calls `Canary.Errors.Ingest.ingest/1` in
   process (no HTTP loopback, no risk of observer-loop). New error
   groups (`ERR-nanoid`) with `first_seen` inside the grace window
   count as a **slow-burn trip** (require confirmation on the next
   poll). Recurrences of pre-existing groups do not trip.
5. **Health targets — `GET /api/v1/health-status`.** Canary probes its
   own configured HTTP targets via `Canary.Health.Manager` (see
   `lib/canary/health/manager.ex`). Any new `up → degraded` or
   `up → down` transition inside the grace window is a **slow-burn
   trip**. `unknown → up` on a freshly-onboarded target is not a trip.
6. **Webhook delivery circuit breaker — `lib/canary/alerter/circuit_breaker.ex`.**
   Opens after 10 consecutive failures per subscription, probes every
   5 minutes, ETS-backed (resets on canary-obs restart, which means a
   fresh deploy zeroes it). Detect via
   `GET /api/v1/webhook-deliveries?status=failed&window=5m`. A breaker
   opening inside the grace window is a **soft trip** — usually a
   downstream consumer is failing, not canary itself, but still worth
   surfacing in the hand-off payload.

## Hard vs. slow-burn asymmetry

| Signal | Class | Confirmation |
|--------|-------|--------------|
| `/healthz` non-200, connection refused, TLS error | Hard | one-shot |
| `/readyz` non-200 sustained > 30s | Hard | one-shot after 30s grace for boot race |
| Fly machine restart loop | Hard | one-shot |
| New self-ingested error group | Slow-burn | 2 consecutive polls |
| New `down`/`degraded` health transition | Slow-burn | 2 consecutive polls |
| Circuit breaker opened | Soft | 2 consecutive polls |

The `/readyz` 30s grace is the only deviation from the generic
hard/soft split and exists specifically because `Canary.Health.Manager`
uses `rescue` in `handle_info(:boot)` to retry in 5s if the DB isn't
ready — documented in `CLAUDE.md` and required for the Ecto sandbox +
prod boot race.

## Contract

**Emits exactly one terminal event per invocation.**

Structured event kinds, wall-clock UTC, one JSON object per line:

- `deploy.monitor.started` — on entry, with `deploy_receipt`, `grace_window_s`, `signals[]`.
- `deploy.monitor.healthy` — per-poll green heartbeat (optional, useful under `/flywheel`).
- `deploy.monitor.tripped { signal, reason, endpoint, samples, recent_errors[] }` — on trip.
- `deploy.monitor.closed { healthy: true|false }` — terminal.

Append to the active `/flywheel` cycle's `cycle.jsonl` when running
under the outer loop; otherwise write to
`.spellbook/monitor/<ulid>.jsonl` in the invoking repo (resolved via
`git rev-parse --show-toplevel`, not this skill's install dir).

Example trip payload:

```json
{
  "schema_version": 1,
  "ts": "2026-04-20T17:02:13Z",
  "kind": "deploy.monitor.tripped",
  "phase": "monitor",
  "refs": ["deploy-receipt:<ref>"],
  "signal": "readyz",
  "endpoint": "https://canary-obs.fly.dev/readyz",
  "reason": "503 for 45s after deploy",
  "samples": [
    {"ts": "2026-04-20T17:01:28Z", "status": 503},
    {"ts": "2026-04-20T17:01:43Z", "status": 503},
    {"ts": "2026-04-20T17:01:58Z", "status": 503},
    {"ts": "2026-04-20T17:02:13Z", "status": 503}
  ],
  "recent_errors": [
    {"id": "ERR-abc123", "group_hash": "...", "message": "...", "first_seen": "2026-04-20T17:01:30Z"}
  ],
  "note": "escalating to /diagnose"
}
```

### Exit codes

| Exit | Meaning |
|------|---------|
| 0 | `deploy.monitor.closed { healthy: true }` — all signals green through the grace window |
| 2 | `deploy.monitor.tripped` — escalating to `/diagnose` (not a failure) |
| 1 | Tooling failure (missing `$CANARY_READ_KEY`, DNS, `flyctl` not on PATH) — `phase.failed` |

Exit 2 is distinct from exit 1 so `/flywheel` routes to `/diagnose` on
2 and to retry-or-abort on 1.

## Authorization

- `/healthz`, `/readyz` → public, no header.
- `/api/v1/query`, `/api/v1/health-status`, `/api/v1/webhook-deliveries`
  → `Authorization: Bearer $CANARY_READ_KEY` (scope `read-only`,
  enforced by `:scope_read` pipeline in `lib/canary_web/router.ex`).
- `flyctl status` → `$FLY_API_TOKEN` in env (same secret the
  `.github/workflows/deploy.yml` job uses).

The bootstrap API key (logged once on first boot — grep
`"Bootstrap API key:"` in Fly logs per `CLAUDE.md`) is `admin` scope;
do NOT use it here. If `$CANARY_READ_KEY` is missing, emit
`phase.failed` and exit 1 rather than falling back to an admin key.

## Control flow

```
/monitor <deploy-receipt-ref> [--grace 5m]
    │
    ▼
  1. Resolve deploy receipt (sha, timestamp). Confirm canary-obs reports
     the new sha in /readyz body or flyctl status image tag.
  2. deadline = receipt_ts + grace_window (default 5m)
  3. Poll every 15s:
       ├── /healthz → hard trip on non-200
       ├── /readyz → hard trip on non-200 sustained > 30s
       ├── flyctl status → hard trip on machine restart loop
       ├── /api/v1/query?service=canary&window=5m → slow-burn on new groups
       ├── /api/v1/health-status → slow-burn on new down/degraded
       └── /api/v1/webhook-deliveries?status=failed → soft on breaker open
  4. Any hard trip → emit deploy.monitor.tripped, hand off /diagnose, exit 2.
     Slow-burn/soft trip confirmed (2 polls) → same.
     All green AND now >= deadline → emit deploy.monitor.closed { healthy: true }, exit 0.
```

## Invocation

```bash
# Outer loop: after /deploy pushes canary-obs
/monitor deploy:<sha>

# Ad-hoc: 10m grace, after a manual `flyctl deploy --app canary-obs --remote-only`
/monitor --grace 10m

# Smoke: no receipt, healthcheck-only against prod
CANARY_READ_KEY=... /monitor
```

## Gotchas

- **Do not duplicate `.github/workflows/uptime-monitor.yml`.** That
  workflow runs every 5 minutes, retries once after 15s, opens a
  GitHub issue labeled `canary-down` on failure, and auto-closes it
  on recovery. `/monitor` is a faster, deploy-scoped watcher — it
  does not file issues. If both fire on the same outage that is
  fine: the workflow handles long-tail incidents, `/monitor` handles
  the deploy window.
- **`/readyz` boot race is expected, briefly.** `Canary.Health.Manager`
  rescues boot failures and retries in 5s. A 503 for the first 15–30s
  after deploy is almost always the DB / supervisor still coming up,
  not a broken deploy. That is why `/readyz` gets a 30s hard-trip grace
  and `/healthz` does not.
- **Canary ingests its own errors in-process.** `Canary.ErrorReporter`
  calls `Canary.Errors.Ingest.ingest/1` directly — not via HTTP. This
  means: (a) a `/healthz` 200 + fresh self-ingested errors is a real
  signal, not a feedback loop; (b) if the ingest path itself is broken,
  errors never appear in `/api/v1/query`, so never rely on "no new
  errors" as a positive signal — pair it with `/readyz`.
- **Circuit breaker state is ETS.** `Canary.Alerter.CircuitBreaker`
  lives in ETS keyed by `webhook_id`, resets on every canary-obs
  restart. A fresh deploy therefore *always* starts with breakers
  closed. "Breaker opened during grace window" is meaningful; "breaker
  currently closed" 30s after deploy is meaningless.
- **Do not diagnose in the trip payload.** Include `signal`, `endpoint`,
  `samples`, and the recent error slab from
  `/api/v1/query?service=canary&window=5m`. Do NOT guess "likely DB
  pool exhaustion" or "probably migration race" — that is `/diagnose`'s
  job, and misleading hints contaminate its first hypothesis.
- **Do not rollback.** Rollback for `canary-obs` is `flyctl releases`
  + redeploy, or in the extreme case the "Nuclear reset" sequence in
  `CLAUDE.md` (stop → ssh → `rm -f /data/canary.db*` → restart).
  Neither belongs in this skill. The operator decides.
- **SQLite WAL + `rm -f` footgun.** If `/monitor` ever finds itself in
  a position where "the DB looks corrupt" is a hypothesis, stop and
  hand off. Deleting `/data/canary.db` on a running machine does
  nothing because WAL keeps the file handle open — this is a
  `/diagnose` problem, not a `/monitor` problem.
- **Never page humans.** The only escalation channel is the
  `deploy.monitor.tripped` event + `/diagnose` hand-off. The uptime
  workflow already files issues for prolonged outages; this skill
  does not.
- **One terminal event per invocation.** Never emit both
  `deploy.monitor.closed` and `deploy.monitor.tripped`. If the poll
  loop somehow sees a late trip after the deadline, trust the first
  terminal condition and exit.
- **The gate (`./bin/validate --strict`) is not a `/monitor` concern.**
  CI runs pre-merge; `/monitor` runs post-deploy. If `/monitor` trips
  and you suspect a bad merge slipped through, open a `/diagnose`
  and let it re-run `./bin/validate` against the offending sha.
