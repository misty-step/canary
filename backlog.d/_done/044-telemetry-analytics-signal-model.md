# Define Canary's telemetry and analytics signal model

Priority: P1
Status: done
Estimate: XL

## Goal
Extend Canary beyond errors and health checks only where it strengthens agentic operations: structured events, lightweight metrics, logs, and product/usage analytics with explicit boundaries, retention, privacy, and OpenTelemetry-aware ingestion strategy.

## Oracle
- [x] A design doc defines which signals Canary owns (`errors`, `health`, `check-ins`, `incidents`, `annotations`, and selected operational events) and which signals it forwards or deliberately leaves to external systems.
- [x] If analytics/events are accepted, the API stores typed event names, service/project ownership, severity, attributes, sampling policy, retention class, and privacy policy without overloading `errors`.
- [x] If metrics/logs are accepted, the design chooses a bounded ingestion path or OpenTelemetry-compatible bridge and names what Canary will not implement.
- [x] Query/report/timeline responses can correlate accepted analytics/metric/log signals with errors and health without unbounded dashboards.
- [x] SDKs expose safe event/metric/log capture APIs with default redaction, sampling, and rate/size limits.
- [x] Agent guidance explains how analytics signals should inform triage without becoming a general BI product.

## Children
1. Write the signal taxonomy and non-goals from first principles.
2. Decide whether to implement native event/metric/log ingestion, an OpenTelemetry bridge, or both in staged form.
3. Add schema/read-model prototypes for typed events with retention/privacy classes.
4. Extend SDKs and integration verification for accepted signals.
5. Add agent report/timeline correlation for accepted analytics signals.
6. Add sampling, quotas, retention, and export/delete policy before broad rollout.

## Notes
- Evidence: README currently positions Canary as replacing Sentry error capture plus Uptime Robot health monitoring, while the user's target includes analytics. The current schema has errors, targets, monitors, incidents, annotations, and webhook delivery, but no first-class analytics/event/metric/log model.
- External exemplar pass: OpenTelemetry's collector pattern separates instrumentation/export from backend handling; Grafana/Better Stack split synthetic monitoring, incident workflows, and status/analytics surfaces. Canary should interoperate or bound scope rather than become an undifferentiated observability warehouse.

## Completion

Delivered on 2026-06-14.

- Canary now owns typed native analytics events only; metrics, logs, and traces
  are explicitly bridge-only until an OpenTelemetry collector/exporter path earns
  a separate implementation.
- `POST /api/v1/events` accepts service-bound ingest keys, validates typed event
  names, severity, object attributes, sampling, retention label, and privacy
  policy, stores events as typed timeline signals, and emits scoped
  `telemetry.event` webhooks without leaking owner fields in public responses.
- Query timelines and reports now correlate recent telemetry events with the
  existing error, health, incident, annotation, and remediation surfaces.
- The TypeScript SDK, CLI, and MCP manifest expose safe event capture helpers;
  CLI ingest config prefers write-capable ingest keys over read-only keys.
- The OpenAPI contract documents the telemetry model, event route, business
  event type, timeline entity type, and operation-level agent guidance.

Evidence:

- `cargo test -p canary-store --locked`
- `cargo test -p canary-server --locked`
- `cargo test -p canary-cli --locked`
- `cargo test -p canary-core --locked`
- `cargo test -p canary-ingest --locked`
- `cargo clippy -p canary-core -p canary-store -p canary-server -p canary-cli --all-targets --locked -- -D warnings`
- `npm --prefix clients/typescript run typecheck`
- `npm --prefix clients/typescript run test:ci`
- `npm --prefix clients/typescript run build`
- `jq empty priv/openapi/openapi.json`
- `git diff --check`
- Local disposable server QA on `127.0.0.1:4317`: raw HTTP event ingest, CLI
  event capture, timeline readback, report `recent_events` readback, and config
  resolution with `ingest_api_key` taking precedence over a bad read key.
- Fresh critic pass verified closure of the webhook ownership, OpenAPI contract,
  CLI key precedence, agent guidance, and retention-overpromise blockers before
  merge.
