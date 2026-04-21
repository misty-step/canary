# Self-observability metrics export

Priority: high
Status: done
Estimate: M

## Goal
Export Canary's own operational metrics so the platform can observe itself.
Telemetry hooks already exist internally; this item wires them to an external
metrics endpoint.

## Non-Goals
- Full Grafana/Prometheus stack deployment — just the export endpoint
- APM-style distributed tracing — Canary is a single-node SQLite app
- Dashboard UI for metrics — consumers scrape the endpoint

## Oracle
- [x] Given the metrics endpoint exists, when a Prometheus scraper hits `GET /metrics`, then it receives valid Prometheus exposition format
- [x] Given errors are being ingested, when metrics are scraped, then `canary_ingest_total`, `canary_ingest_duration_seconds`, and `canary_ingest_errors_total` are present
- [x] Given health probes are running, when metrics are scraped, then `canary_probe_duration_seconds`, `canary_probe_total`, and per-target state gauges are present
- [x] Given webhooks are being delivered, when metrics are scraped, then `canary_webhook_delivery_total` (by status), `canary_webhook_queue_depth`, and `canary_circuit_breaker_state` are present
- [x] Given Oban jobs are running, when metrics are scraped, then queue depth and job duration are present

## Notes
Identified by Thinktank and Codex as a top-priority gap. An observability platform
that cannot observe itself undermines credibility once dogfooding ramps up (007).

Elixir ecosystem approach: `telemetry_metrics_prometheus_core` or
`prom_ex` to bridge existing `:telemetry` events to Prometheus format.
Oban already emits telemetry events; Phoenix and Ecto do too.

## What Was Built
- Added an authenticated `GET /metrics` endpoint that returns Prometheus exposition format and reuses the existing query rate limiting.
- Wired `telemetry_metrics_prometheus_core` into `CanaryWeb.Telemetry` with Canary-specific metric definitions for ingest, probes, webhook delivery, circuit breaker state, queue depth, and Oban job duration.
- Emitted missing telemetry events from the ingest, health-check, and webhook-delivery paths so the exported metrics reflect real runtime behavior instead of static placeholders.
- Added regression coverage for the endpoint contract and the metric families named in this oracle, including healthy and failing probe paths plus counter deltas and target-specific gauge assertions.

## Workarounds
- Test code uses `Canary.Metrics.emit_runtime_metrics/0` so endpoint and exporter checks do not have to wait for the 10s poll interval.
- Health probe metric labels are normalized to `success`/`failure` to keep Prometheus tag cardinality bounded even though the underlying probe result strings are more granular.
- Added `:skip_health_manager_boot` in `config/test.exs` so the always-on health manager does not query SQLite outside the test sandbox during suite boot.

## Verification
- `mix test`
- `mix test test/canary_web/controllers/metrics_controller_test.exs test/canary/metrics_test.exs`
- `mix format --check-formatted`
