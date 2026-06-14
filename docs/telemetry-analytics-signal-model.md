# Telemetry and analytics signal model

Canary is an agentic operations substrate, not a general observability
warehouse. It owns signals that help an agent decide what happened, what is
currently broken, who owns the work, and what bounded evidence should be
replayed before acting.

## Native signals

Canary owns these signals end to end:

- Errors: exception-style failures grouped into error groups and incidents.
- Health targets: HTTP uptime checks and target transition events.
- Check-ins: non-HTTP runtime liveness for cron jobs, workers, desktop apps,
  and scheduled jobs.
- Incidents: correlated active operational units for agents.
- Annotations: evidence, links, and decisions written back by agents.
- Remediation claims: active ownership for automated or human remediation.
- Analytics events: low-volume structured events that explain operational or
  product state relevant to triage.

Analytics events are stored as `telemetry.event` timeline rows. They carry a
consumer event `name`, `service`, `severity`, bounded `summary`, JSON object
`attributes`, `retention_class`, `privacy_policy`, and `sampling_policy`.
They are not stored as fake errors and do not participate in error grouping.

## Bridge-only signals

Canary should interoperate with richer systems rather than rebuild them:

- Metrics: bridge via OpenTelemetry or Prometheus-style exporters later; do
  not store high-cardinality metric series in SQLite.
- Logs: bridge selected error/incident links to a log backend later; do not
  add unbounded log ingestion or search.
- Traces: bridge trace/span IDs through attributes; do not warehouse traces.

The first bridge should be OpenTelemetry-aware at the collector/exporter
boundary: accept or emit a bounded operational event derived from OTel data,
then link back to the external metrics/logs/traces backend through attributes.

## Privacy and retention

The default SDK policy is `privacy_policy=redacted`, `retention_class=standard`,
and `sampling_policy=unsampled`. Producers should scrub PII before sending.
Canary enforces bounded payload size and object-shaped attributes, but it does
not promise full DLP for arbitrary analytics payloads.

Retention classes are persisted policy labels in this slice. Canary returns
them in timeline/report responses so agents and downstream automation can apply
policy, but pruning still follows the existing service-event retention cutoff
until class-aware retention lands.

Retention labels:

- `ephemeral`: short-lived operational breadcrumbs.
- `standard`: default event retention, aligned with timeline retention.
- `audit`: sparse lifecycle evidence worth keeping with incident history.

Privacy policies:

- `redacted`: PII scrubbed before persistence.
- `public`: safe operational metadata.
- `sensitive`: restricted data that should be minimized and avoided unless it
  is required for incident response.

## Agent usage

Agents should treat analytics events as supporting evidence. A
`telemetry.event` can raise or lower suspicion, correlate a deploy or user flow
with an error spike, or explain why a health transition matters. It should not
be the only source of truth for remediation. For active work, agents still read
incidents, claims, errors, health state, and annotations.

Cold-start agents should read `/api/v1/report?window=1h` for current state.
The report includes a bounded `recent_events` list. For durable replay, agents
should poll `/api/v1/timeline?event_type=telemetry.event` and persist the
timeline cursor.

## Non-goals

- General BI dashboards.
- Unbounded event analytics.
- Arbitrary log ingestion.
- High-cardinality metrics storage.
- Trace/span warehousing.
- LLM-generated summaries on the request path.
