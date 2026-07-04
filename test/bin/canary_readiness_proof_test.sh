#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROOF="$ROOT/bin/canary-readiness-proof"
PASS=0
FAIL=0
TMPDIR_TEST="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_TEST"' EXIT

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

assert_json_equals() {
  local json="$1" filter="$2" expected="$3" test_name="$4"
  local actual
  actual="$(jq -r "$filter" <<<"$json")"
  if [ "$actual" = "$expected" ]; then
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $test_name"
    echo "    Filter: $filter"
    echo "    Expected: $expected"
    echo "    Got: $actual"
    FAIL=$((FAIL + 1))
  fi
}

assert_contains() {
  local haystack="$1" needle="$2" test_name="$3"
  if grep -qF -- "$needle" <<<"$haystack"; then
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $test_name"
    echo "    Expected to contain: $needle"
    echo "    Got: $haystack"
    FAIL=$((FAIL + 1))
  fi
}

# A stub `canary` CLI covering doctor/mcp-manifest/mcp-server so this test
# runs offline without a live server or a compiled binary. Behavior is
# switched by the FIXTURE_* env vars each test case sets below.
write_stub_canary() {
  local path="$1"
  cat > "$path" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

has_endpoint=0
has_key=0
subcommand=""
skip_next=0
for arg in "$@"; do
  if [[ $skip_next -eq 1 ]]; then
    skip_next=0
    continue
  fi
  case "$arg" in
    --endpoint) has_endpoint=1; skip_next=1 ;;
    --api-key) has_key=1; skip_next=1 ;;
    --json) ;;
    --*) skip_next=1 ;;
    *)
      if [[ -z "$subcommand" ]]; then
        subcommand="$arg"
      fi
      ;;
  esac
done

TOOL_NAMES='["canary_summary","canary_errors"]'
if [[ "${FIXTURE_MANIFEST_TOOLS:-}" != "" ]]; then
  TOOL_NAMES="$FIXTURE_MANIFEST_TOOLS"
fi

case "$subcommand" in
  doctor)
    if [[ $has_endpoint -eq 1 && $has_key -eq 1 ]]; then
      echo '{"command":"doctor","endpoint":"http://stub","response":{"verdict":{"overall":"healthy"}}}'
      exit 0
    fi
    echo "canary: missing Canary endpoint; set --endpoint, CANARY_ENDPOINT, or config endpoint" >&2
    exit 1
    ;;
  mcp-manifest)
    jq -cn --argjson names "$TOOL_NAMES" '{schema_version: 1, tools: ($names | map({name: ., description: "", input_schema: {}}))}'
    exit 0
    ;;
  mcp-server)
    while IFS= read -r line; do
      id="$(jq -r '.id // empty' <<<"$line")"
      method="$(jq -r '.method // empty' <<<"$line")"
      case "$method" in
        initialize)
          jq -cn --argjson id "$id" '{jsonrpc: "2.0", id: $id, result: {protocolVersion: "2025-11-25", capabilities: {tools: {listChanged: false}}}}'
          ;;
        tools/list)
          jq -cn --argjson id "$id" --argjson names "$TOOL_NAMES" \
            '{jsonrpc: "2.0", id: $id, result: {tools: ($names | map({name: ., inputSchema: {type: "object"}}))}}'
          ;;
        tools/call)
          case "${FIXTURE_MCP_CALL:-ok}" in
            ok)
              jq -cn --argjson id "$id" '{jsonrpc: "2.0", id: $id, result: {content: [{type: "text", text: "{}"}], structuredContent: {}}}'
              ;;
            blocked)
              jq -cn --argjson id "$id" '{jsonrpc: "2.0", id: $id, result: {isError: true, content: [{type: "text", text: "missing Canary endpoint; set --endpoint, CANARY_ENDPOINT, or config endpoint"}]}}'
              ;;
            error)
              jq -cn --argjson id "$id" '{jsonrpc: "2.0", id: $id, result: {isError: true, content: [{type: "text", text: "boom: unexpected tool failure"}]}}'
              ;;
          esac
          ;;
      esac
    done
    ;;
  *)
    echo "stub canary: unsupported subcommand ${args[0]:-}" >&2
    exit 2
    ;;
esac
STUB
  chmod +x "$path"
}

write_stub_dogfood_inventory() {
  local path="$1"
  cat > "$path" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

case "${FIXTURE_DISCOVERY:-blocked}" in
  ok)
    echo '{"summary":{"covered":1,"partial":0,"blocked":0,"ignored":0,"strict_failures":0}}'
    exit 0
    ;;
  blocked)
    echo "dogfood-inventory: manifest not found: /fixture/owned_services.json" >&2
    exit 2
    ;;
  error)
    echo "dogfood-inventory: unexpected internal error" >&2
    exit 3
    ;;
esac
STUB
  chmod +x "$path"
}

write_stub_validate() {
  local path="$1"
  cat > "$path" <<'STUB'
#!/usr/bin/env bash
if [[ "${FIXTURE_VALIDATE:-ok}" == "ok" ]]; then
  echo "fixture validate ok"
  exit 0
fi
echo "fixture validate failed" >&2
exit 1
STUB
  chmod +x "$path"
}

STUB_CANARY="$TMPDIR_TEST/canary"
STUB_DOGFOOD="$TMPDIR_TEST/dogfood-inventory"
STUB_VALIDATE="$TMPDIR_TEST/validate"
write_stub_canary "$STUB_CANARY"
write_stub_dogfood_inventory "$STUB_DOGFOOD"
write_stub_validate "$STUB_VALIDATE"

run_proof() {
  local receipt="$TMPDIR_TEST/receipt-$RANDOM.json"
  local rc=0
  local out
  out="$(
    CANARY_READINESS_CANARY_BIN="$STUB_CANARY" \
    CANARY_READINESS_DOGFOOD_INVENTORY_BIN="$STUB_DOGFOOD" \
    CANARY_READINESS_VALIDATE_BIN="$STUB_VALIDATE" \
    CANARY_READINESS_STATIC_MANIFEST="${CANARY_READINESS_STATIC_MANIFEST:-$TMPDIR_TEST/nonexistent-static-manifest.json}" \
    CANARY_READINESS_RECEIPT="$receipt" \
    "$@" bash "$PROOF" --skip-validate --json 2>"$TMPDIR_TEST/stderr.log"
  )" && rc=0 || rc=$?
  printf '%s\n%s\n%s' "$rc" "$out" "$receipt"
}

echo "Test 1: missing credentials reports blocked fields without failing"
result="$(unset CANARY_ENDPOINT CANARY_API_KEY CANARY_READ_API_KEY; run_proof)"
rc="$(sed -n '1p' <<<"$result")"
body="$(sed -n '2p' <<<"$result")"
assert_exit_code "$rc" "0" "missing credentials exits 0"
assert_json_equals "$body" ".status" "blocked" "status is blocked"
assert_json_equals "$body" ".credentials.endpoint_present" "false" "endpoint reported absent"
assert_json_equals "$body" ".credentials.api_key_present" "false" "api key reported absent"
assert_json_equals "$body" '.blocked_fields | map(.field) | index("endpoint") != null' "true" "endpoint listed as blocked field"
assert_json_equals "$body" '.blocked_fields | map(.field) | index("read_api_key") != null' "true" "read_api_key listed as blocked field"
assert_json_equals "$body" '.blocked_fields[] | select(.field == "read_api_key") | .replacement_command | contains("CANARY_API_KEY")' "true" "read_api_key blocked field names a replacement command"

echo "Test 2: a real API key value never appears in the receipt, only presence"
result="$(CANARY_ENDPOINT=http://stub CANARY_API_KEY="sk_live_should_never_appear_in_receipt" FIXTURE_DISCOVERY=ok FIXTURE_MCP_CALL=ok run_proof)"
body="$(sed -n '2p' <<<"$result")"
if grep -qF "sk_live_should_never_appear_in_receipt" <<<"$body"; then
  echo "  FAIL: receipt leaks the literal key value"
  FAIL=$((FAIL + 1))
else
  echo "  PASS: receipt does not leak the literal key value"
  PASS=$((PASS + 1))
fi
assert_json_equals "$body" ".credentials.api_key_present" "true" "receipt still records that a key was present"

echo "Test 3: full credentials with healthy CLI/MCP/discovery reports ok"
result="$(CANARY_ENDPOINT=http://stub CANARY_API_KEY=sk_live_test_fixture_key FIXTURE_DISCOVERY=ok FIXTURE_MCP_CALL=ok run_proof)"
rc="$(sed -n '1p' <<<"$result")"
body="$(sed -n '2p' <<<"$result")"
assert_exit_code "$rc" "0" "healthy run exits 0"
assert_json_equals "$body" ".status" "ok" "status is ok"
assert_json_equals "$body" ".doctor.status" "ok" "doctor reports ok"
assert_json_equals "$body" ".mcp.probe_status" "ok" "mcp probe reports ok"
assert_json_equals "$body" ".discovery.status" "ok" "discovery reports ok"
assert_json_equals "$body" ".mcp.manifest_stale" "false" "manifest is not stale when tools/list matches mcp-manifest"

echo "Test 4: unconfigured dogfood registry is blocked, not a failure"
result="$(CANARY_ENDPOINT=http://stub CANARY_API_KEY=sk_live_test_fixture_key FIXTURE_DISCOVERY=blocked FIXTURE_MCP_CALL=ok run_proof)"
rc="$(sed -n '1p' <<<"$result")"
body="$(sed -n '2p' <<<"$result")"
assert_exit_code "$rc" "0" "blocked discovery still exits 0"
assert_json_equals "$body" ".status" "blocked" "status is blocked"
assert_json_equals "$body" ".discovery.status" "blocked" "discovery reports blocked"
assert_json_equals "$body" '.blocked_fields[] | select(.field == "dogfood_registry") | .replacement_command | contains("owned_services.json")' "true" "dogfood_registry blocked field names the registry file"

echo "Test 5: an unexpected discovery error fails the proof"
result="$(CANARY_ENDPOINT=http://stub CANARY_API_KEY=sk_live_test_fixture_key FIXTURE_DISCOVERY=error FIXTURE_MCP_CALL=ok run_proof)"
rc="$(sed -n '1p' <<<"$result")"
body="$(sed -n '2p' <<<"$result")"
assert_exit_code "$rc" "1" "unexpected discovery error exits nonzero"
assert_json_equals "$body" ".status" "error" "status is error"

echo "Test 6: a stale generated MCP manifest fails the proof"
cat > "$TMPDIR_TEST/stale-static-manifest.json" <<'JSON'
{"schema_version":1,"tools":[{"name":"canary_summary"},{"name":"canary_errors"}]}
JSON
result="$(CANARY_ENDPOINT=http://stub CANARY_API_KEY=sk_live_test_fixture_key FIXTURE_DISCOVERY=ok FIXTURE_MCP_CALL=ok FIXTURE_MANIFEST_TOOLS='["canary_summary","canary_new_tool_not_in_snapshot"]' CANARY_READINESS_STATIC_MANIFEST="$TMPDIR_TEST/stale-static-manifest.json" run_proof)"
rc="$(sed -n '1p' <<<"$result")"
body="$(sed -n '2p' <<<"$result")"
assert_exit_code "$rc" "1" "stale manifest exits nonzero"
assert_json_equals "$body" ".mcp.manifest_stale" "true" "manifest_stale is true"
assert_json_equals "$body" ".status" "error" "status is error"

echo "Test 7: an unexpected MCP tool-call defect fails the proof"
result="$(CANARY_ENDPOINT=http://stub CANARY_API_KEY=sk_live_test_fixture_key FIXTURE_DISCOVERY=ok FIXTURE_MCP_CALL=error run_proof)"
rc="$(sed -n '1p' <<<"$result")"
body="$(sed -n '2p' <<<"$result")"
assert_exit_code "$rc" "1" "unexpected mcp tool defect exits nonzero"
assert_json_equals "$body" ".mcp.probe_status" "error" "mcp probe reports error"
assert_json_equals "$body" ".status" "error" "status is error"

echo "Test 8: --help documents the proof without requiring credentials"
help_output="$(bash "$PROOF" --help)"
assert_contains "$help_output" "Usage: bin/canary-readiness-proof" "usage line present"
assert_contains "$help_output" "bin/canary doctor" "help mentions doctor step"
assert_contains "$help_output" "bin/validate --fast" "help mentions the repo gate step"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
