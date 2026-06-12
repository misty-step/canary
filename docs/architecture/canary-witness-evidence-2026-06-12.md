# Canary Witness Evidence - 2026-06-12

Backlog item: #037, watch the watchmen.

## Deterministic Local Handling

Command:

```bash
bash -n bin/canary-witness && bash test/bin/canary_witness_test.sh
```

Result:

```text
PASS healthy witness exits zero
PASS healthy receipt status
PASS healthy records healthz
PASS healthy records readyz
PASS healthy records canary query
PASS healthy sends check-in
PASS degraded witness exits nonzero
PASS degraded receipt status
PASS degraded skips check-in
PASS unreachable witness exits nonzero
PASS unreachable receipt status
PASS unreachable records curl exit
PASS malformed witness exits nonzero
PASS malformed receipt status
PASS malformed preserves response body
PASS required missing check-in exits nonzero
PASS missing check-in degrades receipt
PASS missing check-in records reason
18 canary witness tests passed
```

The unreachable case runs with `--require-check-in`, matching the production
workflow invocation.

## Production Configuration

The `canary-watchman` monitor was created on the hosted Canary instance:

```json
{
  "id": "MON-5csw7imodfcq",
  "name": "canary-watchman",
  "service": "canary",
  "mode": "ttl",
  "expected_every_ms": 600000,
  "grace_ms": 120000
}
```

GitHub Actions secrets were configured for the scheduled witness:

```text
CANARY_WITNESS_INGEST_KEY  2026-06-12T00:48:14Z
CANARY_WITNESS_READ_KEY    2026-06-12T00:48:14Z
```

## Live Witness Run

Command shape:

```bash
CANARY_WITNESS_READ_KEY=redacted \
CANARY_WITNESS_INGEST_KEY=redacted \
bin/canary-witness --receipt /tmp/canary-witness-live.json --require-check-in --json
```

Sanitized receipt summary:

```json
{
  "status": "healthy",
  "witness": "canary-watchman",
  "endpoint": "https://canary-obs.fly.dev",
  "observed_at": "2026-06-12T01:10:50Z",
  "healthz": 200,
  "readyz": 200,
  "canary_total_errors": 0,
  "check_in_status": 201,
  "check_in_response": {
    "check_in_id": "CHK-8nx6bhplagft",
    "monitor_id": "MON-5csw7imodfcq",
    "observed_at": "2026-06-12T01:10:50Z",
    "sequence": 4,
    "state": "up"
  }
}
```

## Agent Inspection

Command:

```bash
./bin/canary doctor
```

Result:

```text
endpoint: https://canary-obs.fly.dev
key: CANARY_API_KEY: redacted
key_scope: admin
healthz: ok
readyz: ok
summary: summary: All 5 health surfaces healthy. No errors in the last hour.
services: summary: All 5 health surfaces healthy. No errors in the last hour.
witness: canary-watchman up last_check_in=alive at 2026-06-12T01:10:50Z
canary_errors: summary: 0 errors in canary in the last 1h. 0 unique classes.
incidents: summary: 1 open incident across 1 service. Newest: canary-triage at 2026-03-28T19:02:23.577650Z.
dogfood: covered: 4
worker_readiness: unavailable until #034 lands
```
