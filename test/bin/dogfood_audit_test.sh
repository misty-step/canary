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
  "schema_version": 1,
  "services": [
    {
      "service": "alpha",
      "state": "active",
      "platform": "vercel",
      "production_url": "https://alpha.example",
      "health_url": "https://alpha.example/health",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "No current blocker.",
      "owner": "misty-step",
      "next_action": "Keep enrolled."
    },
    {
      "service": "bravo",
      "state": "active",
      "platform": "fly",
      "production_url": "https://bravo.example",
      "health_url": "https://bravo.example/health",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "No current blocker.",
      "owner": "misty-step",
      "next_action": "Keep enrolled."
    },
    {
      "service": "charlie",
      "state": "pending",
      "platform": "vercel",
      "production_url": "https://charlie.example",
      "health_url": "https://charlie.example/health",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "Waiting on a public health surface.",
      "owner": "misty-step",
      "next_action": "Verify the public health URL."
    },
    {
      "service": "delta",
      "state": "blocked",
      "platform": "vercel",
      "production_url": "https://delta.example",
      "health_url": null,
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "No health route exists.",
      "owner": "misty-step",
      "next_action": "Add a health route."
    },
    {
      "service": "echo",
      "state": "follow_on",
      "platform": "desktop",
      "production_url": null,
      "health_url": null,
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "Desktop app.",
      "owner": "misty-step",
      "next_action": "Use monitor check-ins."
    },
    {
      "service": "canary-self",
      "state": "ignored",
      "platform": "fly",
      "production_url": "https://canary.example",
      "health_url": "https://canary.example/healthz",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "Fixture ignores self target.",
      "owner": "misty-step",
      "next_action": "No fixture action."
    }
  ]
}
JSON
}

write_missing_manifest() {
  local path="$1"
  cat > "$path" <<'JSON'
{
  "schema_version": 1,
  "services": [
    {
      "service": "alpha",
      "state": "active",
      "platform": "vercel",
      "production_url": "https://alpha.example",
      "health_url": "https://alpha.example/health",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "No current blocker.",
      "owner": "misty-step",
      "next_action": "Keep enrolled."
    },
    {
      "service": "missing",
      "state": "active",
      "platform": "vercel",
      "production_url": "https://missing.example",
      "health_url": "https://missing.example/health",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "Target is not enrolled.",
      "owner": "misty-step",
      "next_action": "Enroll the target."
    },
    {
      "service": "canary-self",
      "state": "ignored",
      "platform": "fly",
      "production_url": "https://canary.example",
      "health_url": "https://canary.example/healthz",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "Fixture ignores self target.",
      "owner": "misty-step",
      "next_action": "No fixture action."
    }
  ]
}
JSON
}

write_invalid_manifest() {
  local path="$1"
  cat > "$path" <<'JSON'
{
  "schema_version": 1,
  "services": [
    {
      "service": "alpha",
      "state": "active",
      "platform": "vercel",
      "production_url": "https://alpha.example",
      "health_url": null,
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "Active services require a health URL.",
      "owner": "misty-step",
      "next_action": "Fix schema."
    }
  ]
}
JSON
}

write_stale_manifest() {
  local path="$1"
  cat > "$path" <<'JSON'
{
  "schema_version": 1,
  "services": [
    {
      "service": "alpha",
      "state": "active",
      "platform": "vercel",
      "production_url": "https://alpha.example",
      "health_url": "https://alpha.example/health",
      "last_checked_at": "2026-06-15T00:00:00Z",
      "failure_mode": "Old evidence.",
      "owner": "misty-step",
      "next_action": "Finish backlog items 034 and 041."
    }
  ]
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

assert_json_equals() {
  local json="$1" filter="$2" expected="$3" test_name="$4"
  local actual
  actual="$(jq -r "$filter" <<<"$json")"
  if [ "$actual" = "$expected" ]; then
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $test_name"
    echo "    Expected: $expected"
    echo "    Got: $actual"
    FAIL=$((FAIL + 1))
  fi
}

MANIFEST="$TMPDIR_TEST/manifest.json"
MISSING_MANIFEST="$TMPDIR_TEST/missing-manifest.json"
INVALID_MANIFEST="$TMPDIR_TEST/invalid-manifest.json"
STALE_MANIFEST="$TMPDIR_TEST/stale-manifest.json"
write_manifest "$MANIFEST"
write_missing_manifest "$MISSING_MANIFEST"
write_invalid_manifest "$INVALID_MANIFEST"
write_stale_manifest "$STALE_MANIFEST"

echo "Test 1: dogfood-audit help"
OUTPUT=$(run_and_capture "$DOGFOOD_AUDIT" --help)
assert_contains "$OUTPUT" "Usage: bin/dogfood-audit" "shows dogfood-audit usage"
assert_contains "$OUTPUT" "--json" "documents json output"

echo "Test 2: dogfood-audit renders registry states"
setup_stubbed_curl
OUTPUT=$(CANARY_ENDPOINT=https://canary.example CANARY_API_KEY=sk_test run_and_capture "$DOGFOOD_AUDIT" --manifest "$MANIFEST" --now 2026-06-12T00:00:00Z)
assert_contains "$OUTPUT" "Canary dogfood audit (24h)" "prints report header"
assert_contains "$OUTPUT" "Active services" "prints active section"
assert_contains "$OUTPUT" "alpha" "includes first active service"
assert_contains "$OUTPUT" "bravo" "includes second active service"
assert_contains "$OUTPUT" "pending services" "prints pending section"
assert_contains "$OUTPUT" "charlie" "includes pending service"
assert_contains "$OUTPUT" "blocked services" "prints blocked section"
assert_contains "$OUTPUT" "delta" "includes blocked service"
assert_contains "$OUTPUT" "follow_on services" "prints follow-on section"
assert_contains "$OUTPUT" "echo" "includes follow-on service"
assert_contains "$OUTPUT" "none" "prints empty extra target set"
assert_file_contains "$CURL_LOG" "https://canary.example/api/v1/targets" "fetches live targets"
assert_file_contains "$CURL_LOG" "https://canary.example/api/v1/query?service=alpha&window=24h" "fetches per-service query"

echo "Test 3: dogfood-audit emits machine-readable json"
setup_stubbed_curl
OUTPUT=$(CANARY_ENDPOINT=https://canary.example CANARY_API_KEY=sk_test run_and_capture "$DOGFOOD_AUDIT" --manifest "$MANIFEST" --now 2026-06-12T00:00:00Z --json)
assert_json_equals "$OUTPUT" ".window" "24h" "json includes window"
assert_json_equals "$OUTPUT" ".active_services | length" "2" "json includes active service results"
assert_json_equals "$OUTPUT" ".registry[] | select(.service == \"delta\") | .state" "blocked" "json includes non-active registry states"
assert_json_equals "$OUTPUT" ".extra_targets | length" "0" "json excludes ignored registry target from extras"

echo "Test 4: dogfood-audit strict mode fails when an active target is missing"
setup_stubbed_curl
OUTPUT=$(CANARY_ENDPOINT=https://canary.example CANARY_API_KEY=sk_test run_failure "$DOGFOOD_AUDIT" --manifest "$MISSING_MANIFEST" --now 2026-06-12T00:00:00Z --strict)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "strict mode exits non-zero"
assert_contains "$BODY" "Strict audit failed" "strict mode explains the failure"
assert_contains "$BODY" "missing" "strict output names the missing service state"

echo "Test 5: dogfood-audit rejects invalid registry shape"
setup_stubbed_curl
OUTPUT=$(CANARY_ENDPOINT=https://canary.example CANARY_API_KEY=sk_test run_failure "$DOGFOOD_AUDIT" --manifest "$INVALID_MANIFEST")
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "invalid registry exits non-zero"
assert_contains "$BODY" "Invalid deployed-service registry" "invalid registry explains the schema failure"

echo "Test 6: dogfood-audit strict json reports stale evidence and plural completed-ticket next actions"
setup_stubbed_curl
OUTPUT=$(CANARY_ENDPOINT=https://canary.example CANARY_API_KEY=sk_test run_failure "$DOGFOOD_AUDIT" --manifest "$STALE_MANIFEST" --now 2026-06-14T00:00:00Z --max-evidence-age-hours 24 --strict --json)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "stale strict json exits non-zero"
assert_json_equals "$BODY" ".registry_policy_failures | length" "2" "json includes registry policy failures"
assert_json_equals "$BODY" ".registry_policy_failures[0].kind" "stale_registry_evidence" "json names stale evidence failure"
assert_json_equals "$BODY" ".registry_policy_failures[1].kind" "completed_ticket_next_action" "json names completed-ticket failure"

echo "Test 7: dogfood-audit fails cleanly when curl is unavailable"
setup_path_without_curl
OUTPUT=$(PATH="$MISSING_CURL_PATH" CANARY_ENDPOINT=https://canary.example CANARY_API_KEY=sk_test run_failure "$BASH_BIN" "$DOGFOOD_AUDIT")
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "missing dependency exits non-zero"
assert_contains "$BODY" "Missing required command: curl" "names the missing curl dependency"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
