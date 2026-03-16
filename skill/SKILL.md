# Canary

Query errors, check health, manage targets and webhooks on a self-hosted Canary instance.

## Config

- `CANARY_ENDPOINT` — base URL (e.g. `https://canary-obs.fly.dev`)
- `CANARY_API_KEY` — Bearer token for authentication

All requests: `Authorization: Bearer $CANARY_API_KEY`, `Content-Type: application/json`.

## Query Errors

```bash
# Recent errors for a service (window: 1h|6h|24h|7d|30d, default 1h)
curl -s -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/query?service=<name>&window=1h"

# Paginate with cursor from previous response
curl -s -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/query?service=<name>&window=24h&cursor=<cursor>"

# Group errors by class across all services
curl -s -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/query?group_by=error_class&window=24h"

# Single error detail
curl -s -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/errors/<id>"
```

## Health

```bash
# Overall health status (all targets)
curl -s -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/health-status"

# Check history for a target (window: 1h|6h|24h|7d|30d)
curl -s -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/targets/<id>/checks?window=24h"
```

## Targets

```bash
# List
curl -s -H "Authorization: Bearer $CANARY_API_KEY" "$CANARY_ENDPOINT/api/v1/targets"

# Create
curl -s -X POST -H "Authorization: Bearer $CANARY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name":"my-api","url":"https://api.example.com/healthz","method":"GET","interval_ms":30000,"timeout_ms":5000,"expected_status":200}' \
  "$CANARY_ENDPOINT/api/v1/targets"

# Pause / Resume / Delete
curl -s -X POST -H "Authorization: Bearer $CANARY_API_KEY" "$CANARY_ENDPOINT/api/v1/targets/<id>/pause"
curl -s -X POST -H "Authorization: Bearer $CANARY_API_KEY" "$CANARY_ENDPOINT/api/v1/targets/<id>/resume"
curl -s -X DELETE -H "Authorization: Bearer $CANARY_API_KEY" "$CANARY_ENDPOINT/api/v1/targets/<id>"
```

## Webhooks

```bash
# List
curl -s -H "Authorization: Bearer $CANARY_API_KEY" "$CANARY_ENDPOINT/api/v1/webhooks"

# Create (events: health_check.degraded|down|recovered|tls_expiring, error.new_class|regression)
curl -s -X POST -H "Authorization: Bearer $CANARY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"url":"https://example.com/hook","events":["error.new_class","health_check.down"]}' \
  "$CANARY_ENDPOINT/api/v1/webhooks"

# Test / Delete
curl -s -X POST -H "Authorization: Bearer $CANARY_API_KEY" "$CANARY_ENDPOINT/api/v1/webhooks/<id>/test"
curl -s -X DELETE -H "Authorization: Bearer $CANARY_API_KEY" "$CANARY_ENDPOINT/api/v1/webhooks/<id>"
```

## Errors

All errors use RFC 9457 Problem Details (`type`, `title`, `status`, `detail`).
