# API Key Rotation

Canary API keys are scoped. Use the narrowest key that matches the caller:

- `ingest-only`: `POST /api/v1/errors`
- `read-only`: query, report, timeline, incidents, and read-only health endpoints
- `admin`: target, webhook, onboarding, metrics, annotations write, and key management

Manual rotation is the supported path for now.

## Prerequisites

- An existing `admin` key with permission to create and revoke keys
- A confirmed inventory of the services or operators using the key you want to rotate

## Rotate An Ingest Key

1. Create a replacement key with the same scope:

```bash
curl -X POST https://canary-obs.fly.dev/api/v1/keys \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "billing-api-ingest-2026-04", "scope": "ingest-only"}'
```

2. Update the service secret store or runtime env var to use the new raw key.
3. Trigger a known test error and confirm `POST /api/v1/errors` still succeeds.
4. Revoke the old key after the new key is live everywhere:

```bash
curl -X POST https://canary-obs.fly.dev/api/v1/keys/KEY-old/revoke \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY"
```

## Rotate A Read Or Admin Key

1. Create the replacement with the same scope:

```bash
curl -X POST https://canary-obs.fly.dev/api/v1/keys \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "ops-read-2026-04", "scope": "read-only"}'
```

2. Update dashboards, scripts, or operator tooling to use the new key.
3. Verify the expected access level:
   - `read-only`: `GET /api/v1/report?window=1h` returns `200`
   - `admin`: key management or target management routes return `200`
4. Revoke the previous key only after the replacement is confirmed in use.

## Zero-Downtime Rule

Never revoke first. Canary validates keys immediately, so revoking a key before
all consumers cut over causes avoidable downtime. The safe order is:

1. create replacement
2. roll out replacement
3. verify replacement
4. revoke old key
