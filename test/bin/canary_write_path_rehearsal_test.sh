#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/bin/canary-write-path-rehearsal"
ORIGINAL_PATH="$PATH"
BASH_BIN="$(command -v bash)"
PASS=0
FAIL=0
TMPDIR_TEST=$(mktemp -d)
trap 'rm -rf "$TMPDIR_TEST"' EXIT

setup_stubs() {
  rm -rf "$TMPDIR_TEST/bin" "$TMPDIR_TEST/state"
  mkdir -p "$TMPDIR_TEST/bin" "$TMPDIR_TEST/state"
  export PATH="$TMPDIR_TEST/bin:$ORIGINAL_PATH"
  export CURL_LOG="$TMPDIR_TEST/curl.log"
  export CURL_STATE="$TMPDIR_TEST/state"
  export SSH_LOG="$TMPDIR_TEST/ssh.log"
  : > "$CURL_LOG"
  : > "$SSH_LOG"

  cat > "$TMPDIR_TEST/bin/curl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

method="GET"
output=""
status_format=""
body=""
url=""
auth=""

while (($#)); do
  case "$1" in
    -X)
      method="$2"
      shift 2
      ;;
    -o)
      output="$2"
      shift 2
      ;;
    -w)
      status_format="$2"
      shift 2
      ;;
    -H)
      if [[ "$2" == Authorization:* ]]; then
        auth="${2#Authorization: Bearer }"
      fi
      shift 2
      ;;
    --data)
      body="$2"
      shift 2
      ;;
    -s|-S|-sS)
      shift
      ;;
    *)
      url="$1"
      shift
      ;;
  esac
done

path="${url#http://canary.test}"
printf '%s %s\n' "$method" "$path" >> "${CURL_LOG:?}"

write_response() {
  local status="$1"
  local response="$2"
  printf '%s' "$response" > "$output"
  if [[ "$status_format" == "%{http_code}" ]]; then
    printf '%s' "$status"
  fi
}

case "$method $path" in
  "POST /api/v1/keys")
    touch "$CURL_STATE/key-active"
    raw_key="sk_${CURL_KEY_SCOPE_WORD:-live}_TEST"
    key_prefix="${raw_key%ST}"
    write_response "${CURL_KEY_CREATE_STATUS:-201}" "$(
      jq -cn \
        --arg key "$raw_key" \
        --arg key_prefix "$key_prefix" \
        '{
          id: "KEY-test",
          name: "canary-write-path-test-ingest",
          scope: "ingest-only",
          key: $key,
          key_prefix: $key_prefix,
          detail: ("echoed raw key " + $key),
          created_at: "2026-06-12T20:00:00Z",
          warning: "Store this key securely. It will not be shown again."
        }'
    )"
    ;;
  "GET /api/v1/keys")
    raw_key="sk_${CURL_KEY_SCOPE_WORD:-live}_TEST"
    key_prefix="${raw_key%ST}"
    if [[ -f "$CURL_STATE/key-active" ]]; then
      active=true
      revoked_at=null
    else
      active=false
      revoked_at='"2026-06-12T20:01:00Z"'
    fi
    write_response 200 "$(
      jq -cn \
        --arg key_prefix "$key_prefix" \
        --argjson active "$active" \
        --argjson revoked_at "$revoked_at" \
        '{
          keys: [{
            id: "KEY-test",
            name: "canary-write-path-test-ingest",
            scope: "ingest-only",
            key_prefix: $key_prefix,
            active: $active,
            created_at: "2026-06-12T20:00:00Z",
            revoked_at: $revoked_at
          }]
        }'
    )"
    ;;
  "POST /api/v1/keys/KEY-test/revoke")
    rm -f "$CURL_STATE/key-active"
    write_response 200 '{"status":"revoked"}'
    ;;
  "POST /api/v1/targets")
    expected_target_url="${EXPECT_TARGET_URL:-http://canary.test/healthz}"
    actual_target_url="$(jq -r '.url // ""' <<<"$body")"
    if [[ "$actual_target_url" != "$expected_target_url" ]]; then
      write_response 422 "{\"detail\":\"unexpected target url $actual_target_url\"}"
      exit 0
    fi
    touch "$CURL_STATE/target-active"
    write_response 201 "$(jq -cn --arg url "$actual_target_url" '{id:"TGT-test",name:"canary-write-path-test-target",service:"canary-write-path-test",url:$url,method:"GET",interval_ms:60000,timeout_ms:5000,expected_status:"200",active:true,created_at:"2026-06-12T20:00:00Z"}')"
    ;;
  "GET /api/v1/targets")
    if [[ "$auth" == sk_* ]]; then
      write_response 403 '{"type":"https://canary.local/problems/insufficient_scope","title":"Insufficient Scope","status":403,"detail":"API key scope `ingest-only` cannot access this read endpoint.","code":"insufficient_scope","scope":"ingest-only"}'
      exit 0
    fi
    if [[ -f "$CURL_STATE/target-active" ]]; then
      write_response 200 '{"targets":[{"id":"TGT-test","name":"canary-write-path-test-target","service":"canary-write-path-test","url":"http://canary.test/healthz","active":true}]}'
    else
      write_response 200 '{"targets":[]}'
    fi
    ;;
  "POST /api/v1/targets/TGT-test/pause")
    write_response 200 '{"status":"paused"}'
    ;;
  "POST /api/v1/targets/TGT-test/resume")
    write_response 200 '{"status":"resumed"}'
    ;;
  "DELETE /api/v1/targets/TGT-test")
    rm -f "$CURL_STATE/target-active"
    write_response 204 ''
    ;;
  "POST /api/v1/monitors")
    touch "$CURL_STATE/monitor-active"
    write_response 201 '{"id":"MON-test","name":"canary-write-path-test-monitor","service":"canary-write-path-test","mode":"ttl","expected_every_ms":600000,"grace_ms":120000,"created_at":"2026-06-12T20:00:00Z"}'
    ;;
  "GET /api/v1/monitors")
    if [[ -f "$CURL_STATE/monitor-active" ]]; then
      write_response 200 '{"monitors":[{"id":"MON-test","name":"canary-write-path-test-monitor","service":"canary-write-path-test","mode":"ttl","expected_every_ms":600000,"grace_ms":120000}]}'
    else
      write_response 200 '{"monitors":[]}'
    fi
    ;;
  "POST /api/v1/check-ins")
    if [[ "$auth" == sk_* && ! -f "$CURL_STATE/key-active" ]]; then
      write_response 401 '{"type":"https://canary.local/problems/invalid_api_key","title":"Invalid API Key","status":401,"detail":"Invalid API key.","code":"invalid_api_key"}'
      exit 0
    fi
    write_response 201 '{"monitor_id":"MON-test","check_in_id":"CHK-test","state":"up","observed_at":"2026-06-12T20:00:01Z","sequence":1}'
    ;;
  "DELETE /api/v1/monitors/MON-test")
    rm -f "$CURL_STATE/monitor-active"
    write_response 204 ''
    ;;
  "POST /api/v1/webhooks")
    touch "$CURL_STATE/webhook-active"
    webhook_secret="canary-webhook-""redaction-token"
    write_response 201 "$(
      jq -cn \
        --arg secret "$webhook_secret" \
        '{
          id: "WHK-test",
          url: "https://example.com/hook?canary_rehearsal=test",
          events: ["canary.ping", "error.new_class"],
          secret: $secret,
          message: ("echoed webhook secret " + $secret),
          created_at: "2026-06-12T20:00:00Z"
        }'
    )"
    ;;
  "GET /api/v1/webhooks")
    if [[ -f "$CURL_STATE/webhook-active" ]]; then
      write_response 200 '{"webhooks":[{"id":"WHK-test","url":"https://example.com/hook?canary_rehearsal=test","events":["canary.ping","error.new_class"],"active":true,"created_at":"2026-06-12T20:00:00Z"}]}'
    else
      write_response 200 '{"webhooks":[]}'
    fi
    ;;
  "POST /api/v1/webhooks/WHK-test/test")
    write_response 200 '{"status":"delivered"}'
    ;;
  "DELETE /api/v1/webhooks/WHK-test")
    rm -f "$CURL_STATE/webhook-active"
    write_response 204 ''
    ;;
  "POST /api/v1/errors")
    touch "$CURL_STATE/error-created"
    write_response 201 '{"id":"ERR-test","group_hash":"grp-test","is_new_class":true}'
    ;;
  "GET /api/v1/query?service=canary-write-path-test&window=1h")
    write_response 200 '{"summary":"1 error for canary-write-path-test","service":"canary-write-path-test","window":"1h","total_errors":1,"groups":[{"group_hash":"grp-test","service":"canary-write-path-test","error_class":"CanaryWritePathRehearsal","count":1}],"cursor":null}'
    ;;
  "GET /api/v1/report?window=1h&q=test&limit=5")
    write_response 200 '{"status":"healthy","summary":"ok","targets":[],"monitors":[],"error_groups":[{"group_hash":"grp-test","service":"canary-write-path-test","error_class":"CanaryWritePathRehearsal","count":1}],"search_results":[{"id":"ERR-test","service":"canary-write-path-test","error_class":"CanaryWritePathRehearsal","message":"canary write path rehearsal test","group_hash":"grp-test"}],"incidents":[],"recent_transitions":[],"truncated":false,"cursor":null}'
    ;;
  "GET /api/v1/timeline?service=canary-write-path-test&window=1h&limit=10")
    write_response 200 '{"summary":"Returned 1 timeline events for canary-write-path-test in the last 1h.","returned_count":1,"window":"1h","service":"canary-write-path-test","events":[{"id":"EVT-test","service":"canary-write-path-test","event":"error.new_class","entity_type":"error_group","entity_ref":"grp-test","summary":"new error"}],"cursor":null}'
    ;;
  "GET /api/v1/errors/ERR-test")
    write_response 200 '{"id":"ERR-test","service":"canary-write-path-test","error_class":"CanaryWritePathRehearsal","message":"canary write path rehearsal test","group":{"group_hash":"grp-test"}}'
    ;;
  "GET /api/v1/webhook-deliveries?webhook_id=WHK-test&event=error.new_class&limit=5")
    delivery_status="${CURL_DELIVERY_STATUS:-delivered}"
    write_response 200 "{\"returned_count\":1,\"cursor\":null,\"deliveries\":[{\"delivery_id\":\"DLV-test\",\"webhook_id\":\"WHK-test\",\"event\":\"error.new_class\",\"status\":\"$delivery_status\",\"attempt_count\":1,\"delivered_at\":\"2026-06-12T20:00:02Z\",\"completed_at\":\"2026-06-12T20:00:02Z\"}]}"
    ;;
  "GET /api/v1/webhook-deliveries/DLV-test")
    delivery_status="${CURL_DELIVERY_STATUS:-delivered}"
    write_response 200 "{\"delivery_id\":\"DLV-test\",\"webhook_id\":\"WHK-test\",\"event\":\"error.new_class\",\"status\":\"$delivery_status\",\"attempt_count\":1,\"delivered_at\":\"2026-06-12T20:00:02Z\",\"completed_at\":\"2026-06-12T20:00:02Z\"}"
    ;;
  *)
    write_response 500 "{\"unexpected\":\"$method $path\"}"
    ;;
esac
STUB
  chmod +x "$TMPDIR_TEST/bin/curl"

  cat > "$TMPDIR_TEST/bin/ssh" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${SSH_LOG:?}"
if [[ "$*" == *"systemctl is-active canary.service"* ]]; then
  printf 'active\n'
  exit 0
fi
if [[ "$*" == *"docker inspect canary"* ]]; then
  jq -cn '{
    container_id: "0123456789abcdef",
    image_id: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    name: "/canary",
    state: "running",
    started_at: "2026-07-10T20:00:00Z"
  }'
  exit 0
fi
if [[ "$*" == *"docker exec -i canary sh -eu"* ]]; then
  cat >/dev/null
  printf 'replica /data/canary.db ok\n'
  exit 0
fi
printf 'unexpected ssh invocation: %s\n' "$*" >&2
exit 1
STUB
  chmod +x "$TMPDIR_TEST/bin/ssh"
}

run_and_capture() {
  "$@" 2>&1
}

run_failure() {
  local output

  set +e
  output="$("$@" 2>&1)"
  local rc=$?
  set -e

  printf '%s\n%s' "$rc" "$output"
}

assert_contains() {
  local output="$1" expected="$2" test_name="$3"
  if grep -qF -- "$expected" <<<"$output"; then
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $test_name"
    echo "    Expected to contain: $expected"
    echo "    Got: $output"
    FAIL=$((FAIL + 1))
  fi
}

redact_test_output() {
  local output="$1"
  local raw_key="sk_""live_TEST"
  local raw_key_prefix="${raw_key%ST}"
  local webhook_secret="canary-webhook-""redaction-token"

  output="${output//$raw_key/<redacted-api-key>}"
  output="${output//$raw_key_prefix/<redacted-api-key-prefix>}"
  output="${output//$webhook_secret/<redacted-webhook-secret>}"
  printf '%s' "$output"
}

assert_not_contains() {
  local output="$1" unexpected="$2" test_name="$3" label="${4:-<redacted-forbidden-value>}"
  if grep -qF -- "$unexpected" <<<"$output"; then
    echo "  FAIL: $test_name"
    echo "    Did not expect to contain: $label"
    echo "    Got: $(redact_test_output "$output")"
    FAIL=$((FAIL + 1))
  else
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  fi
}

assert_file_contains() {
  local path="$1" expected="$2" test_name="$3"
  if grep -qF "$expected" "$path"; then
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $test_name"
    echo "    Expected $path to contain: $expected"
    echo "    Got:"
    sed 's/^/      /' "$path"
    FAIL=$((FAIL + 1))
  fi
}

assert_jq() {
  local json="$1" filter="$2" test_name="$3"
  if jq -e "$filter" >/dev/null <<<"$json"; then
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $test_name"
    echo "    jq filter failed: $filter"
    echo "    JSON: $json"
    FAIL=$((FAIL + 1))
  fi
}

assert_exit_code() {
  local actual="$1" expected="$2" test_name="$3"
  if [ "$actual" = "$expected" ]; then
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $test_name"
    echo "    Expected exit code: $expected"
    echo "    Got: $actual"
    FAIL=$((FAIL + 1))
  fi
}

echo "Test 1: help"
OUTPUT=$(run_and_capture "$SCRIPT" --help)
assert_contains "$OUTPUT" "Usage: bin/canary-write-path-rehearsal" "shows usage"

echo "Test 2: missing endpoint fails clearly"
setup_stubs
OUTPUT=$(run_failure env -u CANARY_ENDPOINT -u CANARY_API_KEY PATH="$PATH" "$SCRIPT" --api-key admin)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "missing endpoint exits non-zero"
assert_contains "$BODY" "Missing Canary endpoint" "missing endpoint names fix"

echo "Test 3: missing curl fails clearly"
NO_CURL_PATH="$TMPDIR_TEST/no-curl-path"
rm -rf "$NO_CURL_PATH"
mkdir -p "$NO_CURL_PATH"
OUTPUT=$(run_failure env CANARY_ENDPOINT=http://canary.test CANARY_API_KEY=admin PATH="$NO_CURL_PATH" "$BASH_BIN" "$SCRIPT" --json)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "missing curl exits non-zero"
assert_contains "$BODY" "Missing required command: curl" "missing curl names dependency"

echo "Test 4: stubbed rehearsal emits sanitized JSON receipt"
setup_stubs
OUTPUT=$(env CANARY_ENDPOINT=http://canary.test CANARY_API_KEY=admin EXPECT_TARGET_URL=http://127.0.0.1:4000/healthz "$SCRIPT" --prefix test --webhook-url https://example.com/hook --target-url http://127.0.0.1:4000/healthz --no-dr-status --json)
assert_jq "$OUTPUT" '.status == "ok"' "receipt status ok"
assert_jq "$OUTPUT" '.resources.cleaned_target_id == "TGT-test"' "records target id"
assert_jq "$OUTPUT" '[.steps[] | select(.name == "target_create")][0].response.url == "http://127.0.0.1:4000/healthz"' "uses custom target URL"
assert_jq "$OUTPUT" '.resources.revoked_key_id == "KEY-test"' "records revoked key id"
assert_jq "$OUTPUT" '.resources.immutable_webhook_delivery_id == "DLV-test"' "records delivery id"
assert_jq "$OUTPUT" '[.steps[] | select(.name == "post_cleanup_targets")][0].response.targets == []' "post-cleanup targets empty"
assert_jq "$OUTPUT" '[.steps[] | select(.name == "post_cleanup_monitors")][0].response.monitors == []' "post-cleanup monitors empty"
assert_jq "$OUTPUT" '[.steps[] | select(.name == "post_cleanup_webhooks")][0].response.webhooks == []' "post-cleanup webhooks empty"
assert_jq "$OUTPUT" '.schema_version == 3 and (has("host") | not) and (has("deploy_identity") | not)' "receipt is deployment-topology neutral"
assert_jq "$OUTPUT" '[.steps[] | select(.name == "ingest_key_cannot_read_targets" and .status == 403)] | length == 1' "proves ingest key cannot read admin target list"
assert_jq "$OUTPUT" '[.steps[] | select(.name == "revoked_ingest_key_rejects_check_in" and .status == 401)] | length == 1' "proves revoked ingest key rejects check-in"
RAW_TEST_KEY="sk_""live_TEST"
RAW_TEST_KEY_PREFIX="${RAW_TEST_KEY%ST}"
RAW_WEBHOOK_SECRET="canary-webhook-""redaction-token"
assert_not_contains "$OUTPUT" "$RAW_TEST_KEY" "raw API key is redacted" "<raw API key>"
assert_not_contains "$OUTPUT" "$RAW_TEST_KEY_PREFIX" "API key prefix is redacted" "<API key prefix>"
assert_not_contains "$OUTPUT" "$RAW_WEBHOOK_SECRET" "webhook secret is redacted" "<webhook secret>"
assert_not_contains "$OUTPUT" '"grp-test"' "group hash is redacted from receipt" "<group hash>"

assert_file_contains "$CURL_LOG" "POST /api/v1/errors" "ingests an error"
assert_file_contains "$CURL_LOG" "POST /api/v1/check-ins" "sends monitor check-in"
assert_file_contains "$CURL_LOG" "GET /api/v1/webhook-deliveries?webhook_id=WHK-test&event=error.new_class&limit=5" "queries webhook deliveries"
assert_file_contains "$CURL_LOG" "DELETE /api/v1/targets/TGT-test" "deletes target"
assert_file_contains "$CURL_LOG" "DELETE /api/v1/monitors/MON-test" "deletes monitor"
assert_file_contains "$CURL_LOG" "DELETE /api/v1/webhooks/WHK-test" "deletes webhook"
assert_file_contains "$CURL_LOG" "POST /api/v1/keys/KEY-test/revoke" "revokes key"

echo "Test 5: non-delivered webhook rows fail and clean up"
setup_stubs
OUTPUT=$(run_failure env CANARY_ENDPOINT=http://canary.test CANARY_API_KEY=admin CURL_DELIVERY_STATUS=pending "$SCRIPT" --prefix test --webhook-url https://example.com/hook --poll-attempts 1 --poll-sleep 0 --json)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "pending webhook delivery exits non-zero"
assert_contains "$BODY" "timed out waiting for delivered webhook delivery for WHK-test" "pending delivery explains failure"
assert_file_contains "$CURL_LOG" "DELETE /api/v1/targets/TGT-test" "pending failure deletes target"
assert_file_contains "$CURL_LOG" "DELETE /api/v1/monitors/MON-test" "pending failure deletes monitor"
assert_file_contains "$CURL_LOG" "DELETE /api/v1/webhooks/WHK-test" "pending failure deletes webhook"
assert_file_contains "$CURL_LOG" "POST /api/v1/keys/KEY-test/revoke" "pending failure revokes key"

echo "Test 6: unexpected credential-bearing mutation status redacts and cleans up"
setup_stubs
OUTPUT=$(run_failure env CANARY_ENDPOINT=http://canary.test CANARY_API_KEY=admin CURL_KEY_CREATE_STATUS=500 "$SCRIPT" --prefix test --webhook-url https://example.com/hook --json)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "unexpected key-create status exits non-zero"
assert_contains "$BODY" "api_key_create_ingest expected HTTP 201" "unexpected key-create status explains failure"
RAW_TEST_KEY="sk_""live_TEST"
RAW_TEST_KEY_PREFIX="${RAW_TEST_KEY%ST}"
assert_not_contains "$BODY" "$RAW_TEST_KEY" "unexpected key-create failure redacts raw key" "<raw API key>"
assert_not_contains "$BODY" "$RAW_TEST_KEY_PREFIX" "unexpected key-create failure redacts key prefix" "<API key prefix>"
assert_file_contains "$CURL_LOG" "POST /api/v1/keys/KEY-test/revoke" "unexpected key-create failure revokes key"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
