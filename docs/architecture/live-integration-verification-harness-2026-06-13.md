# Live Integration Verification Harness

Date: 2026-06-13
Backlog: #041

## Goal

Make the canonical Dagger gate prove Canary's production-image integration
surface, not only container boot readiness.

## Gate Command

```bash
PATH=/Users/phaedrus/.local/share/canary-dagger/v0.20.5/bin:$PATH \
  ./bin/dagger call production-image-smoke
```

`production-image-smoke` is called by the checked deterministic lane, so it runs
under `strict` through `deterministic`.

## Proven Path

The harness builds and starts the production Docker image, mints an admin key in
the image, and verifies:

- `/healthz` returns `{"status":"ok"}`.
- `/readyz` returns `{"status":"ready"}`, database `ok`, supervisor `ok`, and
  the five workers `webhook_delivery`, `target_probe`, `monitor_overdue`,
  `retention_prune`, and `tls_scan` started with zero failures.
- The TypeScript SDK builds, ingests an exception through the production Rust
  server, and reads the resulting group back through `/api/v1/query`.
- `bin/canary-write-path-rehearsal` creates disposable admin resources,
  verifies ingest-only key boundaries, creates target/monitor/webhook resources,
  sends a monitor check-in, ingests an error, reads it back through query,
  report, timeline, and error-detail routes, verifies a delivered webhook
  ledger row, and cleans up disposable resources.
- `bin/canary doctor --json` reports worker readiness from `/readyz`.
- `bin/canary mcp-manifest` emits object-shaped input schemas with properties
  for all tools.

## Notes

The Dagger harness uses the Canary service alias for API clients and an explicit
loopback target URL for the target registered inside Canary. Webhook URLs remain
public by design; the smoke uses `https://httpbingo.org/status/204` rather than
weakening webhook egress validation for private CI service aliases.
