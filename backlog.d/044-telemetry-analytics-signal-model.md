# Define Canary's telemetry and analytics signal model

Priority: P1
Status: ready
Estimate: XL

## Goal
Extend Canary beyond errors and health checks only where it strengthens agentic operations: structured events, lightweight metrics, logs, and product/usage analytics with explicit boundaries, retention, privacy, and OpenTelemetry-aware ingestion strategy.

## Oracle
- [ ] A design doc defines which signals Canary owns (`errors`, `health`, `check-ins`, `incidents`, `annotations`, and selected operational events) and which signals it forwards or deliberately leaves to external systems.
- [ ] If analytics/events are accepted, the API stores typed event names, service/project ownership, severity, attributes, sampling policy, retention class, and privacy policy without overloading `errors`.
- [ ] If metrics/logs are accepted, the design chooses a bounded ingestion path or OpenTelemetry-compatible bridge and names what Canary will not implement.
- [ ] Query/report/timeline responses can correlate accepted analytics/metric/log signals with errors and health without unbounded dashboards.
- [ ] SDKs expose safe event/metric/log capture APIs with default redaction, sampling, and rate/size limits.
- [ ] Agent guidance explains how analytics signals should inform triage without becoming a general BI product.

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
