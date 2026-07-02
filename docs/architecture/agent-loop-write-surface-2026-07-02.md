# Agent Loop Write Surface QA - 2026-07-02

## Claim

Canary responders can complete service-bound claim and annotation writeback
through HTTP, the CLI, and the MCP stdio server without admin authority. A
`responder-write` key must be bound to one service and cannot mutate another
service's subject.

## Runtime

- Branch: `factory-062-agent-loop`
- Server: `CANARY_DB_PATH=/tmp/canary-062-live.db PORT=4702 cargo run -q -p canary-server`
- Endpoint: `http://127.0.0.1:4702`
- Public readiness:
  - `GET /healthz` -> `{"status":"ok"}`
  - `GET /readyz` -> `{"status":"ready", ...}`

Raw API keys were captured into shell variables during the run and are not
stored in this receipt.

## Commands

```bash
ADMIN_KEY=$(CANARY_DB_PATH=/tmp/canary-062-live.db \
  cargo run -q -p canary-server -- mint-key --scope admin --name qa-admin \
  2>/tmp/canary-062-mint-admin.err | tail -n 1)

RESPONDER_RESPONSE=$(curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/keys" \
  -H "Authorization: Bearer $ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name":"qa-responder","scope":"responder-write","service":"qa-api"}')

curl -sS -o /tmp/canary-062-unbound.json -w "%{http_code}" \
  -X POST "$CANARY_ENDPOINT/api/v1/keys" \
  -H "Authorization: Bearer $ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name":"bad-responder","scope":"responder-write"}'

curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/targets" \
  -H "Authorization: Bearer $ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"url":"https://example.com/qa-api","name":"QA API","service":"qa-api","interval_ms":60000,"timeout_ms":1000}'

curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/targets" \
  -H "Authorization: Bearer $ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"url":"https://example.com/qa-other","name":"QA Other","service":"qa-other","interval_ms":60000,"timeout_ms":1000}'

CANARY_ENDPOINT="$CANARY_ENDPOINT" CANARY_RESPONDER_KEY="$RESPONDER_KEY" \
  target/debug/canary claims claim \
  --subject-type target --subject-id "$TARGET_API_ID" \
  --owner codex-live --purpose "responder writeback QA" \
  --idempotency-key qa-responder-writeback-062 --json

CANARY_ENDPOINT="$CANARY_ENDPOINT" CANARY_RESPONDER_KEY="$RESPONDER_KEY" \
  target/debug/canary claims transition "$CLAIM_ID" \
  --owner codex-live --state verified \
  --evidence-link docs/architecture/agent-loop-write-surface-2026-07-02.md \
  --json

CANARY_ENDPOINT="$CANARY_ENDPOINT" CANARY_RESPONDER_KEY="$RESPONDER_KEY" \
  target/debug/canary annotations create \
  --subject-type target --subject-id "$TARGET_API_ID" \
  --agent codex-live --action fix-verified \
  --metadata claim_id="$CLAIM_ID" --json

CANARY_ENDPOINT="$CANARY_ENDPOINT" CANARY_RESPONDER_KEY="$RESPONDER_KEY" \
  target/debug/canary annotations list \
  --subject-type target --subject-id "$TARGET_API_ID" --json

curl -sS -o /tmp/canary-062-cross.json -w "%{http_code}" \
  -X POST "$CANARY_ENDPOINT/api/v1/annotations" \
  -H "Authorization: Bearer $RESPONDER_KEY" \
  -H "Content-Type: application/json" \
  -d "{\"subject_type\":\"target\",\"subject_id\":\"$TARGET_OTHER_ID\",\"agent\":\"codex-live\",\"action\":\"cross-service\"}"

printf "%s\n" \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"canary_annotation_create\",\"arguments\":{\"subject_type\":\"target\",\"subject_id\":\"$TARGET_API_ID\",\"agent\":\"mcp-live\",\"action\":\"mcp-writeback\",\"metadata\":{\"claim_id\":\"$CLAIM_ID\"}}}}" \
  "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"canary_annotations_list\",\"arguments\":{\"subject_type\":\"target\",\"subject_id\":\"$TARGET_API_ID\",\"limit\":5}}}" \
  | CANARY_ENDPOINT="$CANARY_ENDPOINT" CANARY_RESPONDER_KEY="$RESPONDER_KEY" \
    target/debug/canary mcp-server
```

## Redacted Result

```json
{
  "endpoint": "http://127.0.0.1:4702",
  "health": {"status": "ok"},
  "ready_status": "ready",
  "responder_key_created": {
    "id": "KEY-i4ia9bmwutvf",
    "scope": "responder-write",
    "service": "qa-api"
  },
  "unbound_responder": {
    "status": 422,
    "code": "validation_error",
    "errors": {"service": ["is required for responder-write keys"]}
  },
  "targets": {
    "api": "TGT-sbxvezded2tv",
    "other": "TGT-gu798ynnek9l"
  },
  "cli": {
    "claim_id": "CLM-vjohc5jlk4by",
    "claim_state": "verified",
    "annotation_id": "ANN-eimkqrxw7okm",
    "annotations_count": 1
  },
  "cross_service_annotation": {
    "status": 403,
    "code": "insufficient_scope",
    "bound_service": "qa-api",
    "requested_service": "qa-other"
  },
  "mcp": {
    "create_id": "ANN-v64fsh9l31pk",
    "create_action": "mcp-writeback",
    "list_count": 2
  }
}
```

## Verdict

PASS for the 062/048 narrow slice: service-bound `responder-write` authority,
CLI claim and annotation writeback, MCP annotation write/list, and cross-service
denial are live-proven.

Not covered here: full 048 rich-context minimization, read-audit events for
rich context reads, browser/public-ingest relay semantics, and webhook receiver
conformance fixtures. Those remain open 048 children.
