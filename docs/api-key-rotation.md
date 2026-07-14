# API Key Rotation

Canary API keys are scoped. Use the narrowest key that matches the caller:

- `ingest-only`: `POST /api/v1/errors`
- `read-only`: query, report, timeline, incidents, and read-only health endpoints;
  service-bound by default
- `responder-write`: service-bound read access plus remediation claim and annotation writeback
- `admin`: target, webhook, onboarding, metrics, key management, and break-glass responder writes

Manual rotation is the supported path for now.

## Prerequisites

- An existing `admin` key with permission to create and revoke keys
- `CANARY_ENDPOINT` set to the operator's Canary instance, for example
  `https://canary.example`
- A confirmed inventory of the services or operators using the key you want to rotate

If the original bootstrap admin key was missed, recover a replacement through
the running image with the no-data-loss path in
[docs/self-host-docker.md](self-host-docker.md).

## Rotate An Ingest Key

1. Create a replacement key with the same scope:

```bash
curl -X POST $CANARY_ENDPOINT/api/v1/keys \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "billing-api-ingest-2026-04", "scope": "ingest-only"}'
```

2. Update the service secret store or runtime env var to use the new raw key.
3. Trigger a known test error and confirm `POST /api/v1/errors` still succeeds.
4. Revoke the old key after the new key is live everywhere:

```bash
curl -X POST $CANARY_ENDPOINT/api/v1/keys/KEY-old/revoke \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY"
```

## Rotate A Read Or Admin Key

1. Create the replacement with the same scope and service binding:

```bash
curl -X POST $CANARY_ENDPOINT/api/v1/keys \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "billing-read-2026-04", "scope": "read-only", "service": "billing"}'
```

An intentionally project-wide reader is an exceptional administrative grant:

```bash
curl -X POST $CANARY_ENDPOINT/api/v1/keys \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "fleet-read-2026-04", "scope": "read-only", "allow_unbound": true}'
```

Omitting both `service` and `allow_unbound: true` is rejected. Existing
unbound read keys created before this contract have no explicit grant and fail
closed after migration; issue and verify a replacement before revoking them.

2. Update dashboards, scripts, or operator tooling to use the new key.
3. Verify the expected access level:
   - `read-only`: `GET /api/v1/report?window=1h` returns `200`
   - `admin`: key management or target management routes return `200`
4. Revoke the previous key only after the replacement is confirmed in use.

## Rotate A Responder-Write Key

`responder-write` keys must be bound to one service. They can read that
service's responder context and write remediation claims or annotations for
subjects owned by the same service; they cannot ingest data, administer Canary,
or cross service boundaries.

1. Create the replacement with the same service binding:

```bash
curl -X POST $CANARY_ENDPOINT/api/v1/keys \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "billing-api-responder-2026-04", "scope": "responder-write", "service": "billing-api"}'
```

2. Store the returned raw key as `CANARY_RESPONDER_KEY` or
   `CANARY_RESPONDER_API_KEY` in the responder runtime.
3. Verify the replacement can read and write only the bound service:
   - `bin/canary incidents get INC-billing-api --json` succeeds for a bound-service incident
   - `bin/canary claims claim --subject-type incident --subject-id INC-billing-api --owner rotation-check --purpose key-rotation --json` succeeds
   - `bin/canary annotations list --subject-type target --subject-id TGT-billing-api --json` succeeds
   - `bin/canary annotations create --subject-type target --subject-id TGT-billing-api --agent rotation-check --action verified --json` succeeds
   - the same create call against another service's target returns RFC 9457 `403`
4. Revoke the previous responder key after the runtime is confirmed on the new
   key.

## Zero-Downtime Rule

Never revoke first. Canary validates keys immediately, so revoking a key before
all consumers cut over causes avoidable downtime. The safe order is:

1. create replacement
2. roll out replacement
3. verify replacement
4. revoke old key
