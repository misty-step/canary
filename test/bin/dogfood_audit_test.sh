#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DOGFOOD_AUDIT="$ROOT/bin/dogfood-audit"
BASH_BIN="$(command -v bash)"
ORIGINAL_PATH="$PATH"
PASS=0
FAIL=0
TMPDIR_TEST="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_TEST"' EXIT

setup_stubbed_curl() {
  rm -rf "$TMPDIR_TEST/bin"
  mkdir -p "$TMPDIR_TEST/bin"
  export PATH="$TMPDIR_TEST/bin:$ORIGINAL_PATH"
  export CURL_LOG="$TMPDIR_TEST/curl.log"

  cat > "$TMPDIR_TEST/bin/curl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >> "${CURL_LOG:?}"
url="${@: -1}"

case "$url" in
  https://canary.example/api/v1/targets)
    cat <<'JSON'
{"targets":[
  {"service":"alpha","url":"https://alpha.example/health"},
  {"service":"bravo","url":"https://bravo.example/health"},
  {"service":"canary-self","url":"https://canary.example/healthz"}
]}
JSON
    ;;
  https://canary.example/api/v1/report?window=24h)
    cat <<'JSON'
{"status":"warning","summary":"3 targets monitored. 2 up. 7 errors across 1 service in the last 24 hours.","targets":[
  {"service":"alpha","url":"https://alpha.example/health","state":"up"},
  {"service":"bravo","url":"https://bravo.example/health","state":"up"},
  {"service":"canary-self","url":"https://canary.example/healthz","state":"up"}
]}
JSON
    ;;
  https://canary.example/api/v1/query?service=alpha\&window=24h)
    cat <<'JSON'
{"service":"alpha","summary":"7 errors in alpha in the last 24h. 1 unique classes.","total_errors":7}
JSON
    ;;
  https://canary.example/api/v1/query?service=bravo\&window=24h)
    cat <<'JSON'
{"service":"bravo","summary":"0 errors in bravo in the last 24h. 0 unique classes.","total_errors":0}
JSON
    ;;
  https://canary.example/api/v1/query?service=missing\&window=24h)
    cat <<'JSON'
{"service":"missing","summary":"0 errors in missing in the last 24h. 0 unique classes.","total_errors":0}
JSON
    ;;
  *)
    printf 'unexpected url: %s\n' "$url" >&2
    exit 99
    ;;
esac
STUB
  chmod +x "$TMPDIR_TEST/bin/curl"
}

setup_path_without_curl() {
  rm -rf "$TMPDIR_TEST/missing-curl-bin"
  mkdir -p "$TMPDIR_TEST/missing-curl-bin"
  ln -sf "$(command -v jq)" "$TMPDIR_TEST/missing-curl-bin/jq"
  MISSING_CURL_PATH="$TMPDIR_TEST/missing-curl-bin"
}

write_manifest() {
  local path="$1"
  cat > "$path" <<'JSON'
{
  "active_services": [
    {"service": "alpha", "target_url": "https://alpha.example/health"},
    {"service": "bravo", "target_url": "https://bravo.example/health"}
  ],
  "pending_services": [
    {
      "service": "charlie",
      "target_url": "https://charlie.example/health",
      "reason": "Waiting on a public health surface."
    }
  ],
  "follow_on_services": [
    {
      "service": "delta",
      "reason": "Desktop app."
    }
  ],
  "ignore_targets": ["canary-self"]
}
JSON
}

write_missing_manifest() {
  local path="$1"
  cat > "$path" <<'JSON'
{
  "active_services": [
    {"service": "alpha", "target_url": "https://alpha.example/health"},
    {"service": "missing", "target_url": "https://missing.example/health"}
  ],
  "pending_services": [],
  "follow_on_services": [],
  "ignore_targets": ["canary-self"]
}
JSON
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
  if echo "$output" | grep -qF "$expected"; then
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $test_name"
    echo "    Expected to contain: $expected"
    echo "    Got: $output"
    FAIL=$((FAIL + 1))
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

MANIFEST="$TMPDIR_TEST/manifest.json"
MISSING_MANIFEST="$TMPDIR_TEST/missing-manifest.json"
write_manifest "$MANIFEST"
write_missing_manifest "$MISSING_MANIFEST"

echo "Test 1: dogfood-audit help"
OUTPUT=$(run_and_capture "$DOGFOOD_AUDIT" --help)
assert_contains "$OUTPUT" "Usage: bin/dogfood-audit" "shows dogfood-audit usage"

echo "Test 2: dogfood-audit renders active, pending, and follow-on sections"
setup_stubbed_curl
OUTPUT=$(CANARY_ENDPOINT=https://canary.example CANARY_API_KEY=sk_test run_and_capture "$DOGFOOD_AUDIT" --manifest "$MANIFEST")
assert_contains "$OUTPUT" "Canary dogfood audit (24h)" "prints report header"
assert_contains "$OUTPUT" "alpha" "includes first active service"
assert_contains "$OUTPUT" "bravo" "includes second active service"
assert_contains "$OUTPUT" "charlie" "includes pending service"
assert_contains "$OUTPUT" "delta" "includes follow-on service"
assert_contains "$OUTPUT" "none" "prints empty extra target set"
assert_file_contains "$CURL_LOG" "https://canary.example/api/v1/targets" "fetches live targets"
assert_file_contains "$CURL_LOG" "https://canary.example/api/v1/query?service=alpha&window=24h" "fetches per-service query"

echo "Test 3: dogfood-audit strict mode fails when an active target is missing"
setup_stubbed_curl
OUTPUT=$(CANARY_ENDPOINT=https://canary.example CANARY_API_KEY=sk_test run_failure "$DOGFOOD_AUDIT" --manifest "$MISSING_MANIFEST" --strict)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "strict mode exits non-zero"
assert_contains "$BODY" "Strict audit failed" "strict mode explains the failure"
assert_contains "$BODY" "missing" "strict output names the missing service state"

echo "Test 4: dogfood-audit fails cleanly when curl is unavailable"
setup_path_without_curl
OUTPUT=$(PATH="$MISSING_CURL_PATH" CANARY_ENDPOINT=https://canary.example CANARY_API_KEY=sk_test run_failure "$BASH_BIN" "$DOGFOOD_AUDIT")
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "missing dependency exits non-zero"
assert_contains "$BODY" "Missing required command: curl" "names the missing curl dependency"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
