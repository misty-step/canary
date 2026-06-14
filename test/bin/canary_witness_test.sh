#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WITNESS="$ROOT/bin/canary-witness"
TMPDIR_TEST="$(mktemp -d)"
PASS=0
FAIL=0
trap 'rm -rf "$TMPDIR_TEST"' EXIT

setup_fake_curl() {
  mkdir -p "$TMPDIR_TEST/bin"
  export CANARY_WITNESS_CURL="$TMPDIR_TEST/bin/fake-curl"

  cat >"$CANARY_WITNESS_CURL" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

output=""
method="GET"
data=""
url=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    -o)
      output="$2"
      shift 2
      ;;
    -w)
      shift 2
      ;;
    -X)
      method="$2"
      shift 2
      ;;
    --data)
      data="$2"
      shift 2
      ;;
    -H|--max-time)
      shift 2
      ;;
    -sS)
      shift
      ;;
    *)
      url="$1"
      shift
      ;;
  esac
done

write_response() {
  local status="$1" latency="$2" body="$3"
  printf '%s' "$body" >"$output"
  printf '%s %s' "$status" "$latency"
}

case "${STUB_SCENARIO:?}:$method:$url" in
  healthy:GET:https://canary.example/healthz)
    write_response 200 0.010 '{"status":"ok"}'
    ;;
  healthy:GET:https://canary.example/readyz)
    write_response 200 0.020 '{"status":"ready","checks":{"database":"ok","supervisor":"ok","workers":[{"name":"webhook_delivery","state":"started","last_success_at":"2026-06-14T02:07:53Z","failure_count":0,"last_error_class":null},{"name":"target_probe","state":"started","last_success_at":"2026-06-14T02:07:55Z","failure_count":0,"last_error_class":null},{"name":"monitor_overdue","state":"started","last_success_at":"2026-06-14T02:07:55Z","failure_count":0,"last_error_class":null},{"name":"retention_prune","state":"started","last_success_at":"2026-06-14T02:07:23Z","failure_count":0,"last_error_class":null},{"name":"tls_scan","state":"started","last_success_at":"2026-06-14T02:07:23Z","failure_count":0,"last_error_class":null}]}}'
    ;;
  healthy:GET:https://canary.example/api/v1/query?service=canary\&window=1h)
    write_response 200 0.030 '{"service":"canary","window":"1h","summary":"0 errors in canary in the last 1h.","total_errors":0,"groups":[]}'
    ;;
  healthy:POST:https://canary.example/api/v1/check-ins)
    if ! jq -e '.monitor == "canary-watchman" and .status == "alive"' <<<"$data" >/dev/null; then
      printf 'invalid check-in payload: %s\n' "$data" >&2
      exit 95
    fi
    write_response 201 0.040 '{"monitor":"canary-watchman","status":"alive"}'
    ;;

  degraded:GET:https://canary.example/healthz)
    write_response 200 0.010 '{"status":"ok"}'
    ;;
  degraded:GET:https://canary.example/readyz)
    write_response 503 0.020 '{"status":"not_ready","checks":{"database":"ok","supervisor":"starting"}}'
    ;;
  degraded:GET:https://canary.example/api/v1/query?service=canary\&window=1h)
    write_response 200 0.030 '{"service":"canary","window":"1h","summary":"0 errors in canary in the last 1h.","total_errors":0,"groups":[]}'
    ;;

  unreachable:GET:https://canary.example/healthz)
    printf '' >"$output"
    printf '000 0.000'
    exit 7
    ;;
  unreachable:GET:https://canary.example/readyz)
    write_response 200 0.020 '{"status":"ready","checks":{"database":"ok","supervisor":"ok","workers":[{"name":"webhook_delivery","state":"started","last_success_at":"2026-06-14T02:07:53Z","failure_count":0,"last_error_class":null},{"name":"target_probe","state":"started","last_success_at":"2026-06-14T02:07:55Z","failure_count":0,"last_error_class":null},{"name":"monitor_overdue","state":"started","last_success_at":"2026-06-14T02:07:55Z","failure_count":0,"last_error_class":null},{"name":"retention_prune","state":"started","last_success_at":"2026-06-14T02:07:23Z","failure_count":0,"last_error_class":null},{"name":"tls_scan","state":"started","last_success_at":"2026-06-14T02:07:23Z","failure_count":0,"last_error_class":null}]}}'
    ;;
  unreachable:GET:https://canary.example/api/v1/query?service=canary\&window=1h)
    write_response 200 0.030 '{"service":"canary","window":"1h","summary":"0 errors in canary in the last 1h.","total_errors":0,"groups":[]}'
    ;;

  malformed:GET:https://canary.example/healthz)
    write_response 200 0.010 'not json'
    ;;
  malformed:GET:https://canary.example/readyz)
    write_response 200 0.020 'not json'
    ;;
  malformed:GET:https://canary.example/api/v1/query?service=canary\&window=1h)
    write_response 200 0.030 'not json'
    ;;

  *)
    printf 'unexpected fake curl request: scenario=%s method=%s url=%s\n' "${STUB_SCENARIO:-}" "$method" "$url" >&2
    exit 99
    ;;
esac
STUB
  chmod +x "$CANARY_WITNESS_CURL"
}

record_pass() {
  PASS=$((PASS + 1))
  printf 'PASS %s\n' "$1"
}

record_fail() {
  FAIL=$((FAIL + 1))
  printf 'FAIL %s\n' "$1" >&2
}

assert_json_equals() {
  local file="$1" filter="$2" expected="$3" message="$4"
  local actual
  actual="$(jq -r "$filter" "$file")"
  if [[ "$actual" == "$expected" ]]; then
    record_pass "$message"
  else
    record_fail "$message (expected '$expected', got '$actual')"
  fi
}

run_success() {
  local message="$1"
  shift
  if "$@" >"$TMPDIR_TEST/canary-witness-test.out" 2>"$TMPDIR_TEST/canary-witness-test.err"; then
    record_pass "$message"
  else
    record_fail "$message"
    cat "$TMPDIR_TEST/canary-witness-test.err" >&2 || true
  fi
}

run_failure() {
  local message="$1"
  shift
  set +e
  "$@" >"$TMPDIR_TEST/canary-witness-test.out" 2>"$TMPDIR_TEST/canary-witness-test.err"
  local status=$?
  set -e
  if [[ "$status" != "0" ]]; then
    record_pass "$message"
  else
    record_fail "$message"
  fi
}

setup_fake_curl

receipt="$TMPDIR_TEST/healthy.json"
STUB_SCENARIO=healthy run_success "healthy witness exits zero" \
  "$WITNESS" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --require-check-in \
    --json
assert_json_equals "$receipt" '.status' 'healthy' "healthy receipt status"
assert_json_equals "$receipt" '.probes.healthz.response.status' 'ok' "healthy records healthz"
assert_json_equals "$receipt" '.probes.readyz.response.status' 'ready' "healthy records readyz"
assert_json_equals "$receipt" '.probes.canary_query.response.total_errors' '0' "healthy records canary query"
assert_json_equals "$receipt" '.check_in.http_status' '201' "healthy sends check-in"

receipt="$TMPDIR_TEST/degraded.json"
STUB_SCENARIO=degraded run_failure "degraded witness exits nonzero" \
  "$WITNESS" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --json
assert_json_equals "$receipt" '.status' 'degraded' "degraded receipt status"
assert_json_equals "$receipt" '.check_in.skipped' 'true' "degraded skips check-in"

receipt="$TMPDIR_TEST/unreachable.json"
STUB_SCENARIO=unreachable run_failure "unreachable witness exits nonzero" \
  "$WITNESS" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --require-check-in \
    --json
assert_json_equals "$receipt" '.status' 'unreachable' "unreachable receipt status"
assert_json_equals "$receipt" '.probes.healthz.curl_exit' '7' "unreachable records curl exit"

receipt="$TMPDIR_TEST/malformed.json"
STUB_SCENARIO=malformed run_failure "malformed witness exits nonzero" \
  "$WITNESS" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --json
assert_json_equals "$receipt" '.status' 'degraded' "malformed receipt status"
assert_json_equals "$receipt" '.probes.healthz.response' 'not json' "malformed preserves response body"

receipt="$TMPDIR_TEST/missing-ingest.json"
run_failure "required missing check-in exits nonzero" \
  env \
    -u CANARY_API_KEY \
    -u CANARY_INGEST_API_KEY \
    -u CANARY_WITNESS_INGEST_KEY \
    STUB_SCENARIO=healthy \
    "$WITNESS" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --receipt "$receipt" \
    --require-check-in \
    --json
assert_json_equals "$receipt" '.status' 'degraded' "missing check-in degrades receipt"
assert_json_equals "$receipt" '.check_in.error' 'missing ingest API key' "missing check-in records reason"

if [[ "$FAIL" -gt 0 ]]; then
  printf '%s canary witness tests failed\n' "$FAIL" >&2
  exit 1
fi

printf '%s canary witness tests passed\n' "$PASS"
