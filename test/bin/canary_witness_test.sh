#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WITNESS=(bash "$ROOT/bin/canary-witness")
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

assert_check_in_payload() {
  if ! jq -e '.monitor == "canary-watchman" and .status == "alive"' <<<"$data" >/dev/null; then
    printf 'invalid check-in payload: %s\n' "$data" >&2
    exit 95
  fi
  if [[ -n "${CHECK_IN_TTL_EXPECTED:-}" ]]; then
    if ! jq -e --argjson ttl "$CHECK_IN_TTL_EXPECTED" '.ttl_ms == $ttl' <<<"$data" >/dev/null; then
      printf 'invalid check-in ttl payload: %s\n' "$data" >&2
      exit 96
    fi
  elif ! jq -e 'has("ttl_ms") | not' <<<"$data" >/dev/null; then
    printf 'expected no ttl_ms in payload: %s\n' "$data" >&2
    exit 97
  fi
}

READY_BODY='{"status":"ready","checks":{"database":"ok","supervisor":"ok","workers":[{"name":"webhook_delivery","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:53Z","last_success_age_ms":250,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":0,"in_flight_count":0,"oldest_due_age_ms":null,"backoff_or_circuit_open":false},{"name":"target_probe","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:55Z","last_success_age_ms":100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":1,"in_flight_count":0,"oldest_due_age_ms":0,"backoff_or_circuit_open":false},{"name":"monitor_overdue","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:55Z","last_success_age_ms":100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":0,"in_flight_count":0,"oldest_due_age_ms":null,"backoff_or_circuit_open":false},{"name":"retention_prune","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:23Z","last_success_age_ms":32100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":1,"in_flight_count":0,"oldest_due_age_ms":null,"backoff_or_circuit_open":false},{"name":"tls_scan","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:23Z","last_success_age_ms":32100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":2,"in_flight_count":0,"oldest_due_age_ms":null,"backoff_or_circuit_open":false}]}}'
PRESSURED_READY_BODY='{"status":"ready","checks":{"database":"ok","supervisor":"ok","workers":[{"name":"webhook_delivery","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:53Z","last_success_age_ms":250,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":0,"in_flight_count":0,"oldest_due_age_ms":null,"backoff_or_circuit_open":false},{"name":"target_probe","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:55Z","last_success_age_ms":100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":1,"in_flight_count":0,"oldest_due_age_ms":0,"backoff_or_circuit_open":false},{"name":"monitor_overdue","state":"started","health":"pressured","last_success_at":"2026-06-14T02:07:55Z","last_success_age_ms":100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":1,"in_flight_count":0,"oldest_due_age_ms":7200000,"backoff_or_circuit_open":false},{"name":"retention_prune","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:23Z","last_success_age_ms":32100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":1,"in_flight_count":0,"oldest_due_age_ms":null,"backoff_or_circuit_open":false},{"name":"tls_scan","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:23Z","last_success_age_ms":32100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":2,"in_flight_count":0,"oldest_due_age_ms":null,"backoff_or_circuit_open":false}]}}'
NOT_READY_WORKERS_BODY='{"status":"not_ready","checks":{"database":"ok","supervisor":"ok","workers":[{"name":"webhook_delivery","state":"started","health":"pressured","last_success_at":"2026-06-14T02:07:53Z","last_success_age_ms":250,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":12,"in_flight_count":0,"oldest_due_age_ms":7200000,"backoff_or_circuit_open":true},{"name":"target_probe","state":"started","health":"failing","last_success_at":"2026-06-14T02:07:55Z","last_success_age_ms":100,"failure_count":3,"consecutive_failures":3,"last_error_class":"runtime_error","due_count":1,"in_flight_count":0,"oldest_due_age_ms":0,"backoff_or_circuit_open":false},{"name":"monitor_overdue","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:55Z","last_success_age_ms":100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":0,"in_flight_count":0,"oldest_due_age_ms":null,"backoff_or_circuit_open":false},{"name":"retention_prune","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:23Z","last_success_age_ms":32100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":1,"in_flight_count":0,"oldest_due_age_ms":null,"backoff_or_circuit_open":false},{"name":"tls_scan","state":"started","health":"ok","last_success_at":"2026-06-14T02:07:23Z","last_success_age_ms":32100,"failure_count":0,"consecutive_failures":0,"last_error_class":null,"due_count":2,"in_flight_count":0,"oldest_due_age_ms":null,"backoff_or_circuit_open":false}]}}'

case "${STUB_SCENARIO:?}:$method:$url" in
  healthy:GET:https://canary.example/healthz)
    write_response 200 0.010 '{"status":"ok"}'
    ;;
  healthy:GET:https://canary.example/readyz)
    write_response 200 0.020 "$READY_BODY"
    ;;
  healthy:GET:https://canary.example/api/v1/query?service=canary\&window=1h)
    write_response 200 0.030 '{"service":"canary","window":"1h","summary":"0 errors in canary in the last 1h.","total_errors":0,"groups":[]}'
    ;;
  healthy:POST:https://canary.example/api/v1/check-ins)
    assert_check_in_payload
    write_response 201 0.040 '{"monitor":"canary-watchman","status":"alive"}'
    ;;

  slow-query:GET:https://canary.example/healthz)
    write_response 200 0.010 '{"status":"ok"}'
    ;;
  slow-query:GET:https://canary.example/readyz)
    write_response 200 0.020 "$READY_BODY"
    ;;
  slow-query:GET:https://canary.example/api/v1/query?service=canary\&window=1h)
    write_response 200 1.250 '{"service":"canary","window":"1h","summary":"0 errors in canary in the last 1h.","total_errors":0,"groups":[]}'
    ;;

  slow-check-in:GET:https://canary.example/healthz)
    write_response 200 0.010 '{"status":"ok"}'
    ;;
  slow-check-in:GET:https://canary.example/readyz)
    write_response 200 0.020 "$READY_BODY"
    ;;
  slow-check-in:GET:https://canary.example/api/v1/query?service=canary\&window=1h)
    write_response 200 0.030 '{"service":"canary","window":"1h","summary":"0 errors in canary in the last 1h.","total_errors":0,"groups":[]}'
    ;;
  slow-check-in:POST:https://canary.example/api/v1/check-ins)
    assert_check_in_payload
    write_response 201 1.250 '{"monitor":"canary-watchman","status":"alive"}'
    ;;

  pressured:GET:https://canary.example/healthz)
    write_response 200 0.010 '{"status":"ok"}'
    ;;
  pressured:GET:https://canary.example/readyz)
    write_response 200 0.020 "$PRESSURED_READY_BODY"
    ;;
  pressured:GET:https://canary.example/api/v1/query?service=canary\&window=1h)
    write_response 200 0.030 '{"service":"canary","window":"1h","summary":"0 errors in canary in the last 1h.","total_errors":0,"groups":[]}'
    ;;
  pressured:POST:https://canary.example/api/v1/check-ins)
    assert_check_in_payload
    write_response 201 0.040 '{"monitor":"canary-watchman","status":"alive"}'
    ;;

  self-pressured:GET:https://canary.example/healthz)
    write_response 200 0.010 '{"status":"ok"}'
    ;;
  self-pressured:GET:https://canary.example/readyz)
    write_response 200 0.020 "$PRESSURED_READY_BODY"
    ;;
  self-pressured:GET:https://canary.example/api/v1/query?service=canary\&window=1h)
    write_response 200 0.030 '{"service":"canary","window":"1h","summary":"0 errors in canary in the last 1h.","total_errors":0,"groups":[]}'
    ;;
  self-pressured:POST:https://canary.example/api/v1/check-ins)
    assert_check_in_payload
    write_response 201 0.040 '{"monitor":"canary-watchman","status":"alive"}'
    ;;

  notready-workers:GET:https://canary.example/healthz)
    write_response 200 0.010 '{"status":"ok"}'
    ;;
  notready-workers:GET:https://canary.example/readyz)
    write_response 503 0.020 "$NOT_READY_WORKERS_BODY"
    ;;
  notready-workers:GET:https://canary.example/api/v1/query?service=canary\&window=1h)
    write_response 200 0.030 '{"service":"canary","window":"1h","summary":"0 errors in canary in the last 1h.","total_errors":0,"groups":[]}'
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
    write_response 200 0.020 "$READY_BODY"
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

assert_stderr_contains() {
  local expected="$1" message="$2"
  if grep -qF -- "$expected" "$TMPDIR_TEST/canary-witness-test.err"; then
    record_pass "$message"
  else
    record_fail "$message"
    cat "$TMPDIR_TEST/canary-witness-test.err" >&2 || true
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

run_failure "missing endpoint exits nonzero" \
  env \
    -u CANARY_ENDPOINT \
    -u CANARY_WITNESS_ENDPOINT \
    STUB_SCENARIO=healthy \
    "${WITNESS[@]}" \
    --read-api-key read-key \
    --receipt "$TMPDIR_TEST/missing-endpoint.json" \
    --json
assert_stderr_contains \
  "canary-witness: missing endpoint: pass --endpoint or set CANARY_WITNESS_ENDPOINT/CANARY_ENDPOINT" \
  "missing endpoint names configuration"

receipt="$TMPDIR_TEST/healthy.json"
run_success "healthy witness exits zero" \
  env \
    STUB_SCENARIO=healthy \
    "${WITNESS[@]}" \
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

receipt="$TMPDIR_TEST/healthy-latency.json"
run_success "healthy witness stays within an explicit latency budget" \
  env \
    STUB_SCENARIO=healthy \
    "${WITNESS[@]}" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --max-latency-ms 500 \
    --json
assert_json_equals "$receipt" '.max_latency_ms' '500' "latency receipt records the budget"
assert_json_equals "$receipt" '.latency_breaches | length' '0' "healthy probes stay within the budget"

receipt="$TMPDIR_TEST/slow-query.json"
run_failure "slow HTTP 200 witness exits nonzero" \
  env \
    STUB_SCENARIO=slow-query \
    "${WITNESS[@]}" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --receipt "$receipt" \
    --max-latency-ms 500 \
    --json
assert_json_equals "$receipt" '.status' 'degraded' "slow HTTP 200 degrades the receipt"
assert_json_equals "$receipt" '.probes.canary_query.within_latency_budget' 'false' "slow query is outside the budget"
assert_json_equals "$receipt" '.latency_breaches[0].name' 'canary-query' "slow receipt names the breached probe"

receipt="$TMPDIR_TEST/slow-check-in.json"
run_failure "slow check-in HTTP 201 witness exits nonzero" \
  env \
    STUB_SCENARIO=slow-check-in \
    "${WITNESS[@]}" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --max-latency-ms 500 \
    --require-check-in \
    --json
assert_json_equals "$receipt" '.latency_breaches[0].name' 'check-in' "slow receipt names the check-in breach"

run_failure "zero latency budget is rejected" \
  env \
    STUB_SCENARIO=healthy \
    "${WITNESS[@]}" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --receipt "$TMPDIR_TEST/invalid-latency.json" \
    --max-latency-ms 0 \
    --json
assert_stderr_contains \
  "canary-witness: max-latency-ms must be a positive integer number of milliseconds" \
  "invalid latency budget names the bad input"

receipt="$TMPDIR_TEST/healthy-ttl.json"
run_success "healthy witness sends configured ttl" \
  env \
    CHECK_IN_TTL_EXPECTED=7200000 \
    STUB_SCENARIO=healthy \
    CANARY_WITNESS_TTL_MS=7200000 \
    "${WITNESS[@]}" \
      --endpoint https://canary.example \
      --read-api-key read-key \
      --ingest-api-key ingest-key \
      --receipt "$receipt" \
      --require-check-in \
      --json
assert_json_equals "$receipt" '.status' 'healthy' "healthy ttl receipt status"
assert_json_equals "$receipt" '.check_in.http_status' '201' "healthy ttl sends check-in"

receipt="$TMPDIR_TEST/pressured.json"
run_success "pressured ready (unrelated monitor overdue) witness exits zero" \
  env \
    STUB_SCENARIO=pressured \
    "${WITNESS[@]}" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --require-check-in \
    --json
assert_json_equals "$receipt" '.status' 'healthy' "pressured ready by an unrelated monitor still reports healthy"
assert_json_equals "$receipt" '.probes.readyz.response.checks.workers[] | select(.name == "monitor_overdue") | .health' 'pressured' "pressured ready records worker pressure"
assert_json_equals "$receipt" '.alert_plane.status' 'impaired' "pressured ready still records alert-plane impairment in the receipt"
assert_json_equals "$receipt" '.alert_plane.workers[] | select(.name == "monitor_overdue") | .health' 'pressured' "pressured ready names impaired worker"
assert_json_equals "$receipt" '.check_in.skipped' 'false' "pressured ready by an unrelated monitor still sends check-in"
assert_json_equals "$receipt" '.check_in.http_status' '201' "pressured ready check-in succeeds"
assert_json_equals "$receipt" '.self_heal_check_in' 'true' "pressured ready by another monitor is out of scope and counts as self-heal"

receipt="$TMPDIR_TEST/self-pressured.json"
run_success "self-pressured (witness's own monitor overdue) witness exits zero" \
  env \
    STUB_SCENARIO=self-pressured \
    "${WITNESS[@]}" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --require-check-in \
    --json
assert_json_equals "$receipt" '.status' 'healthy' "self-pressured receipt status becomes healthy"
assert_json_equals "$receipt" '.alert_plane.status' 'impaired' "self-pressured still records alert-plane impairment in the receipt"
assert_json_equals "$receipt" '.self_heal_check_in' 'true' "self-pressured triggers self-heal"
assert_json_equals "$receipt" '.check_in.skipped' 'false' "self-pressured still sends check-in"
assert_json_equals "$receipt" '.check_in.http_status' '201' "self-pressured check-in succeeds"

receipt="$TMPDIR_TEST/notready-workers.json"
run_failure "not-ready worker witness exits nonzero" \
  env \
    STUB_SCENARIO=notready-workers \
    "${WITNESS[@]}" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --json
assert_json_equals "$receipt" '.status' 'degraded' "not-ready worker receipt status"
assert_json_equals "$receipt" '.probes.readyz.response.status' 'not_ready' "not-ready worker preserves readyz body"
assert_json_equals "$receipt" '.alert_plane.status' 'impaired' "not-ready worker records alert-plane impairment"
assert_json_equals "$receipt" '.alert_plane.reasons[] | select(. == "webhook_delivery backoff_or_circuit_open")' 'webhook_delivery backoff_or_circuit_open' "not-ready worker names backoff"
assert_json_equals "$receipt" '.alert_plane.reasons[] | select(. == "target_probe failing")' 'target_probe failing' "not-ready worker names failing worker"
assert_json_equals "$receipt" '.check_in.skipped' 'true' "not-ready worker skips check-in"

receipt="$TMPDIR_TEST/degraded.json"
run_failure "degraded witness exits nonzero" \
  env \
    STUB_SCENARIO=degraded \
    "${WITNESS[@]}" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --json
assert_json_equals "$receipt" '.status' 'degraded' "degraded receipt status"
assert_json_equals "$receipt" '.check_in.skipped' 'true' "degraded skips check-in"

receipt="$TMPDIR_TEST/unreachable.json"
run_failure "unreachable witness exits nonzero" \
  env \
    STUB_SCENARIO=unreachable \
    "${WITNESS[@]}" \
    --endpoint https://canary.example \
    --read-api-key read-key \
    --ingest-api-key ingest-key \
    --receipt "$receipt" \
    --require-check-in \
    --json
assert_json_equals "$receipt" '.status' 'unreachable' "unreachable receipt status"
assert_json_equals "$receipt" '.probes.healthz.curl_exit' '7' "unreachable records curl exit"

receipt="$TMPDIR_TEST/malformed.json"
run_failure "malformed witness exits nonzero" \
  env \
    STUB_SCENARIO=malformed \
    "${WITNESS[@]}" \
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
    "${WITNESS[@]}" \
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
