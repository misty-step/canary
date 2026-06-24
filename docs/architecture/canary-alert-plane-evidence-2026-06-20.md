# Canary Alert-Plane Evidence - 2026-06-20

## Scope

Backlog #047 first deliverable: prove alert-plane health separately from route
readiness before adding SLO or burn-rate math.

This slice keeps `GET /readyz` route readiness unchanged: a `pressured` worker
can still appear inside a ready response. `bin/canary doctor` and
`bin/canary-witness` now grade that same worker state as impaired
alert-plane health.

## Induced Impairment

Command:

```bash
bash -n bin/canary-witness && bash test/bin/canary_witness_test.sh
```

Result:

```text
PASS pressured ready witness exits nonzero
PASS pressured ready receipt status
PASS pressured ready records worker pressure
PASS pressured ready records alert-plane impairment
PASS pressured ready names impaired worker
PASS pressured ready skips check-in
PASS not-ready worker witness exits nonzero
PASS not-ready worker preserves readyz body
PASS not-ready worker records alert-plane impairment
PASS not-ready worker names backoff
PASS not-ready worker names failing worker
PASS not-ready worker skips check-in
34 canary witness tests passed
```

The induced fixture returns HTTP 200 `/readyz` with
`response.status == "ready"` and `monitor_overdue.health == "pressured"`.
The suite also includes a `503` `/readyz` body with stale/failing worker
snapshots so the alert plane still returns structured reasons when route
readiness is already down. The pressured witness receipt records:

```json
{
  "status": "degraded",
  "alert_plane": {
    "status": "impaired",
    "impaired_workers": 1,
    "reasons": ["monitor_overdue pressured"]
  },
  "check_in": {
    "skipped": true,
    "error": "self signals were not healthy"
  }
}
```

## CLI Contract

Command:

```bash
cargo test -p canary-cli --locked
```

Result:

```text
28 unit tests passed
12 fixture tests passed
```

The focused regression is
`doctor_verdict_degrades_for_alert_plane_pressure_even_when_readyz_is_ready`.
It proves a ready route plus a pressured worker yields:

```json
{
  "overall": "degraded",
  "alert_plane": {
    "status": "impaired",
    "reasons": ["monitor_overdue pressured"]
  }
}
```

The follow-up regression is `alert_plane_uses_not_ready_worker_snapshots`. It
proves a non-2xx `/readyz` response that still carries worker snapshots yields
structured reasons such as `webhook_delivery backoff_or_circuit_open` and
`target_probe failing`.

## Live Doctor Readback

Command:

```bash
bin/canary doctor --json > /tmp/canary-alert-plane-doctor-live.json
jq '{overall: .response.verdict.overall, alert_plane: .response.alert_plane, verdict_alert_plane: .response.verdict.alert_plane, worker_readiness: {status: .response.worker_readiness.status, worker_count: .response.worker_readiness.worker_count, failing_workers: .response.worker_readiness.failing_workers, pressured_workers: .response.worker_readiness.pressured_workers}, next_operator_action: .response.verdict.next_operator_action}' /tmp/canary-alert-plane-doctor-live.json
```

Result:

```json
{
  "overall": "healthy",
  "alert_plane": {
    "available": true,
    "impaired_workers": 0,
    "reasons": [],
    "status": "healthy",
    "worker_count": 5,
    "workers": []
  },
  "verdict_alert_plane": {
    "available": true,
    "impaired_workers": 0,
    "reasons": [],
    "status": "healthy",
    "worker_count": 5,
    "workers": []
  },
  "worker_readiness": {
    "status": "ready",
    "worker_count": 5,
    "failing_workers": 0,
    "pressured_workers": 0
  },
  "next_operator_action": "No runtime blocker; run `bin/canary dogfood audit --strict --json` and close the reported coverage gaps."
}
```

## Production-Image Rehearsal

Command:

```bash
PATH="/tmp/canary-dagger-0.20.5.OgaFYN:$PATH" ./bin/dagger call production-image-alert-plane-rehearsal
```

Red result before the Dagger entrypoint existed:

```text
unknown command "production-image-alert-plane-rehearsal" for "dagger call"
```

Green result after wiring the rehearsal:

```json
{
  "route_ready": "ready",
  "alert_plane": {
    "available": true,
    "impaired_workers": 1,
    "reasons": ["monitor_overdue pressured"],
    "status": "impaired",
    "worker_count": 5,
    "workers": [
      {
        "name": "monitor_overdue",
        "state": "started",
        "health": "pressured",
        "due_count": 1,
        "oldest_due_age_ms": 620818,
        "reason": "monitor_overdue pressured"
      }
    ]
  },
  "verdict": "degraded"
}
```

The same Dagger run also executed `bin/canary-witness` against the production
image service. The witness exited nonzero and recorded:

```json
{
  "status": "degraded",
  "alert_plane": {
    "status": "impaired",
    "reasons": ["monitor_overdue pressured"]
  },
  "check_in": {
    "skipped": true,
    "error": "self signals were not healthy"
  }
}
```

The rehearsal uses the disposable production-image smoke database. It creates a
unique TTL monitor through the admin API, writes one backdated check-in through
the ingest API, waits for the real `monitor_overdue` worker to observe the
overdue deadline, then requires both doctor and witness to degrade while
`/readyz` remains route-ready.

Integrated production-image smoke:

```bash
PATH="/tmp/canary-dagger-0.20.5.OgaFYN:$PATH" ./bin/dagger call production-image-smoke
```

Result: passed. The integrated smoke ran the existing SDK write/readback,
write-path rehearsal, doctor/MCP smoke, and the new production-image
alert-plane rehearsal. The final rehearsal summary reported
`route_ready: "ready"`, `alert_plane.status: "impaired"`,
`monitor_overdue.health: "pressured"`, and witness `status: "degraded"`.

## Validation

Focused commands:

```bash
cargo fmt --all --check
git diff --check
cargo test -p canary-cli --locked
bash -n bin/canary-witness && bash test/bin/canary_witness_test.sh
env PATH="/tmp/canary-dagger-0.20.5.OgaFYN:$PATH" ./bin/dagger call production-image-alert-plane-rehearsal
env PATH="/tmp/canary-dagger-0.20.5.OgaFYN:$PATH" bash ./bin/validate --fast
```

Result: all passed. The fast gate includes the Debian `jq` 1.6 witness lane,
which caught and verified the portable alert-plane JSON expression.

Earlier first-slice strict gate:

```bash
env PATH="/tmp/canary-dagger-0.20.5.OgaFYN:$PATH" bash ./bin/validate --strict
```

Result:

```text
✔ .strict(...): Void 6m8s
```

The final strict run used the repo-pinned Dagger `v0.20.5` binary after removing
a stale `v0.21.6` engine that had blocked the earlier default `dagger check`
wrapper.

The full branch strict gate still runs before the #047 PR and squash merge. The
current production-image rehearsal is already wired into `productionImageSmoke`,
which strict reaches through `deterministic`.

## Remaining #047 Scope

This does not implement SLO configuration, multi-window burn-rate summaries, or
the report/CLI/MCP burn-rate surfaces. Those remain children of #047 after the
alert-plane impairment oracle and timestamp skew policy.
