# Responder Context Safety

This document is the contract for rich context that Canary returns to external
responders. The current shipped slice covers incident detail reads through
`GET /api/v1/incidents/{id}`.

The goal is least privilege, minimization, and replayability: a responder wakes
from a webhook, claims or annotates with a service-bound `responder-write` key,
reads one bounded incident context envelope for the same service, acts outside
Canary, and leaves durable audit evidence.

## Authority

| Credential | Incident context read behavior |
| --- | --- |
| `responder-write` with `service=<name>` | May read incident context for the bound service. |
| `responder-write` without a service binding | Rejected with RFC 9457 `insufficient_scope`. |
| `responder-write` bound to a different service | Rejected with RFC 9457 `insufficient_scope`; response includes `bound_service` and `requested_service`. |
| `read-only` with `service=<name>` | May read incident detail only for the bound service. |
| `read-only` with explicit `allow_unbound=true` grant | May read incident detail across the key owner scope. |
| legacy unbound `read-only` without an explicit grant | Rejected with RFC 9457 `insufficient_scope`; rotate the key. |
| `admin` | Break-glass read; context response is still redacted, but no responder read-audit event is written. |

Responder keys are not tenant-wide read keys. They are service-bound automation
credentials for claim, annotation, transition, and incident-context loops.

## Incident Context Envelope

Every incident detail response includes `context_envelope`:

```json
{
  "schema": "canary.responder_context.incident.v1",
  "tenant_id": "TENANT-...",
  "project_id": "PROJECT-...",
  "service": "service-name",
  "subject": {
    "type": "incident",
    "id": "INC-..."
  },
  "retention": {
    "class": "audit",
    "audit_event": "responder.context_read"
  },
  "privacy_policy": {
    "classification": "redacted",
    "redaction_rules": [
      "bearer_token",
      "canary_api_key",
      "jwt",
      "aws_access_key",
      "private_key_block",
      "provider_token",
      "credential_database_uri",
      "email",
      "sensitive_key_value"
    ],
    "max_string_chars": 1024,
    "max_metadata_bytes": 4096,
    "max_evidence_link_chars": 512
  },
  "bounds": {
    "signals": {
      "returned": 1,
      "max": 25,
      "truncated": false
    },
    "annotations": {
      "returned": 1,
      "max": 20,
      "truncated": false
    },
    "claims": {
      "returned": 1,
      "max": 20
    },
    "recent_timeline_events": {
      "returned": 5,
      "max": 5
    }
  },
  "audit_event_id": "EVT-..."
}
```

The incident context schema is an allowlist over the existing incident detail
read model: `summary`, `incident`, `signals`, `signals_truncated`,
`annotations`, `annotations_truncated`, `claims`, `recent_timeline_events`,
`action_brief`, and `context_envelope`.

## Redaction Policy

| Rule | Input shape | Output |
| --- | --- | --- |
| `bearer_token` | `Bearer <token-like-value>` | `Bearer [REDACTED]` |
| `canary_api_key` | `sk_live_*` or `sk_test_*` | `[CANARY_API_KEY]` |
| `jwt` | Three base64url-like segments beginning with `eyJ` | `[JWT]` |
| `aws_access_key` | `AKIA*` or `ASIA*` access-key ID | `[AWS_ACCESS_KEY]` |
| `private_key_block` | PEM private-key block | `[PRIVATE_KEY]` |
| `provider_token` | Declared GitHub, Slack, OpenAI, Anthropic, OpenRouter, Google, Hugging Face, GitLab, or npm token shape | `[PROVIDER_TOKEN]` |
| `credential_database_uri` | PostgreSQL, MySQL, MongoDB, or Redis URI with user info | Scheme plus `[REDACTED]@` |
| `email` | Email-shaped strings | `[EMAIL]` |
| `sensitive_key_value` | Object keys or assignment names such as `authorization`, `cookie`, `password`, `secret`, `token`, `api_key`, `access_token`, or `refresh_token` | `[REDACTED]` or `name=[REDACTED]` |
| `max_string_chars` | Any string longer than 1024 characters | First 1024 characters plus ` [TRUNCATED]` |
| `max_metadata_bytes` | Annotation metadata JSON larger than 4096 serialized bytes | Truncation marker with original and limit byte counts |
| `max_evidence_link_chars` | Claim evidence link longer than 512 characters | First 512 characters plus ` [TRUNCATED]` |

The durable store may preserve raw submitted annotations, telemetry attributes,
and claim evidence. Redaction happens at the responder HTTP boundary before the
payload is serialized.

## Read Audit

Successful non-admin incident context reads insert one telemetry event:

```json
{
  "event_type": "telemetry.event",
  "signal_name": "responder.context_read",
  "retention_class": "audit",
  "privacy_policy": "redacted",
  "attributes": {
    "reader": {
      "key_id": "KEY-...",
      "key_name": "responder-name",
      "scope": "responder-write",
      "service": "service-name"
    },
    "route": "GET /api/v1/incidents/{id}",
    "subject": {
      "type": "incident",
      "id": "INC-...",
      "service": "service-name"
    },
    "context_envelope": {
      "schema": "canary.responder_context.incident.v1",
      "privacy_policy": "redacted",
      "retention_class": "audit"
    },
    "read_at": "2026-07-04T00:00:00Z"
  }
}
```

The audit event records who read which context and when. It does not include
the response payload.

## Residuals

This slice does not claim the full backlog.d/048 surface:

- Target and monitor responder context envelopes still need equivalent
  minimization and read audit.
- Browser/public-ingest credential semantics remain a separate safety gate
  before client-side capture is promoted.
- Webhook receiver fixtures exist for timestamp validation, delivery-id dedupe,
  and timeline replay, but this slice does not expand them.
- `GET /api/v1/errors/{id}` remains a raw drill-down route for broader read
  authority. Responder agents should start with incident context and only fall
  through when the envelope reports truncation or the workflow explicitly needs
  raw stack traces.
