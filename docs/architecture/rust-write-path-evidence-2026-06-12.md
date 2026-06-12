# Rust write-path evidence: 2026-06-12

This packet records a live production-shaped write-path rehearsal against the
deployed Rust `canary-server` on Fly. It complements the 2026-06-06 cutover
packet: that packet proved production serving plus public/read routes; this
one proves the admin, ingest, target, monitor, webhook, worker-backed delivery,
query/report/timeline readback, cleanup, and DR-status paths.

## Service under test

- Fly app: `canary-obs`
- Endpoint: `https://canary-obs.fly.dev`
- Deployed commit: `8066373b55108f662bf08158dfd74c25561a9fd4`
- Deployed image: `registry.fly.io/canary-obs:deployment-01KTYR1CH63W842CPD6TM775FW`
- Image digest: `sha256:bf4d29b374f423cdd93e926ea6d2d0a5a8a2c49bfbf2d726bd2c357e9c1c2c65`
- Machine: `78407d7f515008`
- Fly release version: `76`
- Region: `iad`

`flyctl status --app canary-obs --json` reported the machine `started`, host
status `ok`, and both service checks passing:

```json
[
  {
    "name": "servicecheck-01-http-4000",
    "status": "passing",
    "output": "{\"status\":\"ready\",\"checks\":{\"database\":\"ok\",\"supervisor\":\"ok\"}}"
  },
  {
    "name": "servicecheck-00-http-4000",
    "status": "passing",
    "output": "{\"status\":\"ok\"}"
  }
]
```

## Rehearsal command

The command used the local admin key only as an Authorization header. The JSON
receipt redacts one-time API keys, key prefixes, webhook secrets, key hashes,
and error group hashes before printing.

```bash
bin/canary-write-path-rehearsal \
  --prefix 20260612-live-rust-write-path-final \
  --json \
  | tee /tmp/canary-write-path-rehearsal-2026-06-12-final.json \
  | jq '{status, prefix, deploy_commit: .deploy_identity.machines[0].image_ref.commit, deploy_digest: .deploy_identity.machines[0].image_ref.digest, machine: .deploy_identity.machines[0].id, resources, cleanup_note, step_count: (.steps | length)}'
```

Receipt summary:

```json
{
  "status": "ok",
  "prefix": "20260612-live-rust-write-path-final",
  "deploy_commit": "8066373b55108f662bf08158dfd74c25561a9fd4",
  "deploy_digest": "sha256:bf4d29b374f423cdd93e926ea6d2d0a5a8a2c49bfbf2d726bd2c357e9c1c2c65",
  "machine": "78407d7f515008",
  "resources": {
    "service": "canary-write-path-20260612-live-rust-write-path-final",
    "immutable_error_id": "ERR-kc7o7ryv121g",
    "cleaned_target_id": "TGT-2cr0ze6iwcfx",
    "cleaned_monitor_id": "MON-k2843ev95yz9",
    "cleaned_webhook_id": "WHK-opehar1c54yt",
    "revoked_key_id": "KEY-vy07y1z3576z",
    "immutable_webhook_delivery_id": "DLV-stwp8h56qgxd"
  },
  "cleanup_note": "Targets, monitors, and webhooks are deleted. The temporary ingest key is revoked and intentionally remains as an inactive audit row. Error and webhook-delivery rows are immutable evidence.",
  "step_count": 30
}
```

Full redacted receipt:
[`docs/architecture/rust-write-path-receipt-2026-06-12.json`](rust-write-path-receipt-2026-06-12.json).
That checked-in artifact contains the complete `.steps[]` response bodies from
the run, including query/report/timeline readback and post-cleanup
targets/monitors/webhooks/keys responses.

## Write-path coverage

All HTTP statuses below came from the live receipt.

| Step | Route or command | Result |
|---|---|---|
| Fly deploy identity | `flyctl status --app canary-obs --json` | `deployed`, machine `78407d7f515008`, image digest `sha256:bf4d29b374f423cdd93e926ea6d2d0a5a8a2c49bfbf2d726bd2c357e9c1c2c65`, commit `8066373b55108f662bf08158dfd74c25561a9fd4` |
| API key mint | `POST /api/v1/keys` | `201`, created `KEY-vy07y1z3576z` with `ingest-only` scope |
| Ingest key read denial | `GET /api/v1/targets` with the ingest-only key | `403`, `insufficient_scope` |
| Target create | `POST /api/v1/targets` | `201`, created `TGT-2cr0ze6iwcfx` for the rehearsal service |
| Target query | `GET /api/v1/targets` | `200`, listed the disposable target before cleanup |
| Target pause/resume | `POST /api/v1/targets/{id}/pause`, `POST /api/v1/targets/{id}/resume` | both `200` |
| Monitor create | `POST /api/v1/monitors` | `201`, created `MON-k2843ev95yz9` |
| Monitor query | `GET /api/v1/monitors` | `200`, listed the disposable monitor before cleanup |
| Monitor check-in | `POST /api/v1/check-ins` | `201`, returned state `up`, sequence `1` |
| Webhook create | `POST /api/v1/webhooks` | `201`, created `WHK-opehar1c54yt` for `canary.ping` and `error.new_class` |
| Webhook test | `POST /api/v1/webhooks/{id}/test` | `200`, returned `delivered` |
| Error ingest | `POST /api/v1/errors` | `201`, created `ERR-kc7o7ryv121g`, new class `true` |
| Error query readback | `GET /api/v1/query?service=...&window=1h` | `200`, `total_errors: 1` for the rehearsal service |
| Report readback | `GET /api/v1/report?window=1h&q=...&limit=5` | `200`, included the rehearsal error group, search result, target transition, monitor transition, and incident |
| Timeline readback | `GET /api/v1/timeline?service=...&window=1h&limit=10` | `200`, returned 5 events for the rehearsal service |
| Error detail readback | `GET /api/v1/errors/ERR-kc7o7ryv121g` | `200`, returned the expected service and class summary |
| Webhook delivery page | `GET /api/v1/webhook-deliveries?webhook_id=...&event=error.new_class&limit=5` | `200`, returned `DLV-stwp8h56qgxd` |
| Webhook delivery lookup | `GET /api/v1/webhook-deliveries/DLV-stwp8h56qgxd` | `200`, status `delivered`, attempt count `1` |
| DR status | `NO_COLOR=1 bin/dr-status --app canary-obs` | exit `0`, Litestream status `ok` for `/data/canary.db` |

DR-status excerpt:

```text
database         status  local txid        wal size
/data/canary.db  ok      0000000000089857  5.5 MB
```

## Cleanup proof

The rehearsal deleted disposable HTTP resources and revoked the temporary
ingest key before returning `ok`.

| Cleanup step | Result |
|---|---|
| `DELETE /api/v1/targets/TGT-2cr0ze6iwcfx` | `204` |
| `DELETE /api/v1/monitors/MON-k2843ev95yz9` | `204` |
| `DELETE /api/v1/webhooks/WHK-opehar1c54yt` | `204` |
| `POST /api/v1/keys/KEY-vy07y1z3576z/revoke` | `200`, `{"status":"revoked"}` |
| `POST /api/v1/check-ins` with the revoked ingest key | `401`, `invalid_api_key` |
| `GET /api/v1/targets` after cleanup | `200`, no `TGT-2cr0ze6iwcfx` |
| `GET /api/v1/monitors` after cleanup | `200`, no `MON-k2843ev95yz9` |
| `GET /api/v1/webhooks` after cleanup | `200`, no `WHK-opehar1c54yt` |
| `GET /api/v1/keys` after cleanup | `200`, `KEY-vy07y1z3576z` present only as inactive audit row |

The error row, incident row, timeline events, and webhook-delivery row are not
deleted. They are the immutable observability evidence for this run:

- Error: `ERR-kc7o7ryv121g`
- Incident opened for service: `canary-write-path-20260612-live-rust-write-path-final`
- Delivery: `DLV-stwp8h56qgxd`

## Residual risks

- This packet deliberately creates one active error group for the rehearsal
  service. Aggregate reports can show a warning until the normal query window
  no longer includes that event; `service=canary` error checks are unaffected.
- The webhook proof used `https://httpbingo.org/status/204` as a public 2xx
  receiver. `bin/canary-write-path-rehearsal --webhook-url <url>` can replay
  the same proof against another receiver if that service changes behavior.
- This proves live HTTP write/read paths and the webhook delivery worker for one
  delivery. It does not prove retention-prune or TLS-expiry workers under
  long-running production conditions; #034 owns the worker readiness oracle.
