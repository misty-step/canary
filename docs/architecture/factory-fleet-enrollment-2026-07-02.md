# Factory Fleet Enrollment Evidence - 2026-07-02 UTC

## Claim

The Misty Step Canary hub is now the monitoring half of the current Factory
composition. Canary, bastion, powder, and the Bitterblossom plane each have
explicit coverage status, live readback, and a repeatable enrollment path.

Run window: `2026-07-02T01:06Z` through `2026-07-02T01:14Z`.

## Hub Configuration

The hub is `canary-obs` at `https://canary-obs.fly.dev`. Private Fly fleet
targets require the hub runtime flag below so Fly 6PN addresses are accepted by
the target validator:

```bash
flyctl secrets set --app canary-obs ALLOW_PRIVATE_TARGETS=true
```

This is hub operator configuration, not consumer-repo configuration.

## Coverage Matrix

| Repo / runtime | Status | Health / uptime readback | Error or check-in path | Next app-lane action |
|---|---|---|---|---|
| Canary | Integrated | `canary-self` target `TGT-5gowpivgtuvw` read back `up` at `https://canary-obs.fly.dev/healthz`. | `canary-watchman` monitor `MON-5csw7imodfcq` read back `up`; query readback returned `0` canary errors in the last hour. | Keep witness scheduled after each deploy. |
| Bastion router | Integrated | Non-HTTP uptime monitor `bastion-router` read back `up`. | `MON-vsaibxn27k8p` last check-in `alive` at `2026-07-02T01:13:45.31256448Z`. | Resident bastion lane keeps the heartbeat flowing. |
| Powder | Integrated | Target `TGT-s637fnhh257c` read back `up` for `http://powder.internal:4000/healthz` at `2026-07-02T01:10:55.804934739Z`. | Proof monitor `MON-8en7bvbq9a5g`, check-in `CHK-8iwuhyz986ai`, state `up`; query readback returned `0` errors in the last hour. | Move the proof heartbeat into the powder app lane. Canary did not edit the resident powder branch. |
| Bitterblossom plane | Integrated | Target `TGT-zzjlrcj90ynk` read back `up` for `https://bitterblossom-plane.fly.dev/health` at `2026-07-02T01:11:27.25369684Z`. | Proof monitor `MON-btdkn8d6uo9r`, check-in `CHK-6pis4h07vb36`, state `up`; query readback returned `0` errors in the last hour. | Move the proof heartbeat into the BB plane lane. Canary did not mutate the BB repo. |

No other Factory runtime app was named by the 2026-07-01 operator decisions for
this epic. Non-runtime repos and app-side heartbeat patches are intentionally
deferred to their resident lanes.

## Live Writes

Powder target enrollment initially failed with:

```text
target resolved to non-global address fdaa:45:6cb6:a7b:7d2:d9ab:7ac8:2
```

After setting `ALLOW_PRIVATE_TARGETS=true` on the hub, the same enrollment
succeeded:

```json
{
  "id": "TGT-s637fnhh257c",
  "service": "powder",
  "url": "http://powder.internal:4000/healthz",
  "interval_ms": 60000,
  "expected_status": "200",
  "active": true
}
```

Bitterblossom-plane target enrollment:

```json
{
  "id": "TGT-zzjlrcj90ynk",
  "service": "bitterblossom-plane",
  "url": "https://bitterblossom-plane.fly.dev/health",
  "interval_ms": 60000,
  "expected_status": "200",
  "active": true
}
```

Proof monitor creation and check-ins:

```json
[
  {
    "monitor_id": "MON-8en7bvbq9a5g",
    "check_in_id": "CHK-8iwuhyz986ai",
    "service": "powder",
    "state": "up",
    "observed_at": "2026-07-02T01:11:10.651517749Z",
    "sequence": 1
  },
  {
    "monitor_id": "MON-btdkn8d6uo9r",
    "check_in_id": "CHK-6pis4h07vb36",
    "service": "bitterblossom-plane",
    "state": "up",
    "observed_at": "2026-07-02T01:11:10.912502549Z",
    "sequence": 1
  }
]
```

## Live Readback

`GET /api/v1/report?window=24h` showed the new targets and fleet monitors:

```json
{
  "targets": [
    {
      "service": "bitterblossom-plane",
      "url": "https://bitterblossom-plane.fly.dev/health",
      "state": "up",
      "last_checked_at": "2026-07-02T01:11:27.25369684Z"
    },
    {
      "service": "powder",
      "url": "http://powder.internal:4000/healthz",
      "state": "up",
      "last_checked_at": "2026-07-02T01:10:55.804934739Z"
    }
  ],
  "monitors": [
    {
      "service": "bastion",
      "name": "bastion-router",
      "state": "up",
      "last_check_in_status": "alive",
      "last_check_in_at": "2026-07-02T01:13:45.31256448Z"
    },
    {
      "service": "bitterblossom-plane",
      "name": "bitterblossom-plane-fleet-heartbeat",
      "state": "up",
      "last_check_in_status": "alive",
      "last_check_in_at": "2026-07-02T01:11:10.912502549Z"
    },
    {
      "service": "powder",
      "name": "powder-fleet-heartbeat",
      "state": "up",
      "last_check_in_status": "alive",
      "last_check_in_at": "2026-07-02T01:11:10.651517749Z"
    }
  ]
}
```

Service query readback:

```json
[
  {
    "service": "powder",
    "total_errors": 0,
    "summary": "0 errors in powder in the last 1h. 0 unique classes."
  },
  {
    "service": "bitterblossom-plane",
    "total_errors": 0,
    "summary": "0 errors in bitterblossom-plane in the last 1h. 0 unique classes."
  },
  {
    "service": "canary",
    "total_errors": 0,
    "summary": "0 errors in canary in the last 1h. 0 unique classes."
  }
]
```

## Strict Dogfood Proof

The updated audit was run with a temporary instance-local manifest containing
`canary-self`, `canary`, `bastion`, `powder`, and `bitterblossom-plane`.
Result:

```json
{
  "strict_failures": 0,
  "active": [
    {"service":"canary-self","target":"present","url":"yes","health":"up","monitor":"n/a","total_errors":0},
    {"service":"canary","target":"n/a","url":"n/a","health":"n/a","monitor":"up","total_errors":1},
    {"service":"bastion","target":"n/a","url":"n/a","health":"n/a","monitor":"up","total_errors":0},
    {"service":"powder","target":"present","url":"yes","health":"up","monitor":"up","total_errors":0},
    {"service":"bitterblossom-plane","target":"present","url":"yes","health":"up","monitor":"up","total_errors":0}
  ]
}
```

The manifest was temporary because Misty Step service names are instance-local
operator state, not checked-in clean-room examples.

## Residuals

- Powder and BB check-ins are proof monitors with 24-hour TTLs. The app lanes
  should replace them with scoped app-owned ingest keys and recurring
  heartbeats.
- `bin/canary doctor --json` in this 061 worktree reported `degraded` because
  the alert plane had `monitor_overdue pressured` and the worktree did not have
  an instance-local `.canary/dogfood/owned_services.json`. The service-specific
  fleet dogfood audit above passed with live readback.
- Bitterblossom private Flycast health at
  `http://bitterblossom-plane.internal:8080/health` refused from the Canary
  machine during enrollment, so the public health URL was enrolled. The BB lane
  owns any private-bind change.
