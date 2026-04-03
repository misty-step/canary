# Self-observability metrics export

Priority: high
Status: ready
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
- [ ] Given the metrics endpoint exists, when a Prometheus scraper hits `GET /metrics`, then it receives valid Prometheus exposition format
- [ ] Given errors are being ingested, when metrics are scraped, then `canary_ingest_total`, `canary_ingest_duration_seconds`, and `canary_ingest_errors_total` are present
- [ ] Given health probes are running, when metrics are scraped, then `canary_probe_duration_seconds`, `canary_probe_total`, and per-target state gauges are present
- [ ] Given webhooks are being delivered, when metrics are scraped, then `canary_webhook_delivery_total` (by status), `canary_webhook_queue_depth`, and `canary_circuit_breaker_state` are present
- [ ] Given Oban jobs are running, when metrics are scraped, then queue depth and job duration are present

## Notes
Identified by Thinktank and Codex as a top-priority gap. An observability platform
that cannot observe itself undermines credibility once dogfooding ramps up (007).

Elixir ecosystem approach: `telemetry_metrics_prometheus_core` or
`prom_ex` to bridge existing `:telemetry` events to Prometheus format.
Oban already emits telemetry events; Phoenix and Ecto do too.
