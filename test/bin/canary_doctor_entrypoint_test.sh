#!/bin/bash
# Regression test for bin/canary-doctor-entrypoint.sh (canary-913).
# Proves CANARY_API_KEY never reaches cargo/canary-cli as a literal argv
# value — it must be picked up from the inherited process environment only.
set -e

ENTRYPOINT="$(cd "$(dirname "$0")/../.." && pwd)/bin/canary-doctor-entrypoint.sh"
ORIGINAL_PATH="$PATH"
PASS=0
FAIL=0
TMPDIR_TEST=$(mktemp -d)
trap 'rm -rf "$TMPDIR_TEST"' EXIT

SECRET_VALUE="sk_test_do_not_leak_9f3a7c2e"

reset_env() {
  unset CANARY_API_KEY
  unset CANARY_DOCTOR_ENDPOINT
}

setup_stubs() {
  export PATH="$TMPDIR_TEST/bin:$ORIGINAL_PATH"
  mkdir -p "$TMPDIR_TEST/bin"
  export CARGO_LOG="$TMPDIR_TEST/cargo.log"
  : > "$CARGO_LOG"
  cat > "$TMPDIR_TEST/bin/cargo" << 'STUB'
#!/bin/bash
# Log full argv exactly as this process received it (not the parent
# shell's unexpanded script text) — this is what `ps`/`docker top` would
# show for the real cargo/canary-cli process.
printf '%s\n' "$*" >> "${CARGO_LOG:?}"
exit 0
STUB
  chmod +x "$TMPDIR_TEST/bin/cargo"
}

run_entrypoint() {
  bash "$ENTRYPOINT" 2>&1
}

run_entrypoint_failure() {
  local output
  set +e
  output=$(run_entrypoint)
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

assert_not_contains() {
  local output="$1" unexpected="$2" test_name="$3"
  if grep -qF -- "$unexpected" <<<"$output"; then
    echo "  FAIL: $test_name"
    echo "    Expected NOT to contain: $unexpected"
    echo "    Got: $output"
    FAIL=$((FAIL + 1))
  else
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
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

# --- Test 1: missing key fails closed before ever invoking cargo ---
echo "Test 1: CANARY_API_KEY unset fails closed"
reset_env
setup_stubs
OUTPUT=$(run_entrypoint_failure)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "2" \
  "exits non-zero when CANARY_API_KEY is unset"
assert_contains "$BODY" "set CANARY_API_KEY to an admin or read-only key" \
  "reports the missing-key message"
if [ -s "$CARGO_LOG" ]; then
  echo "  FAIL: does not invoke cargo when the key is missing"
  FAIL=$((FAIL + 1))
else
  echo "  PASS: does not invoke cargo when the key is missing"
  PASS=$((PASS + 1))
fi

# --- Test 2: key present -> the raw value never appears in cargo's argv ---
echo "Test 2: CANARY_API_KEY set is never passed as a literal argv value"
reset_env
setup_stubs
export CANARY_API_KEY="$SECRET_VALUE"
run_entrypoint > /dev/null
CARGO_ARGV="$(cat "$CARGO_LOG")"
assert_not_contains "$CARGO_ARGV" "$SECRET_VALUE" \
  "cargo/canary-cli argv does not contain the raw key value"
assert_not_contains "$CARGO_ARGV" "--api-key" \
  "cargo/canary-cli is never invoked with an --api-key flag"

# --- Test 3: doctor still runs against the resolved endpoint ---
echo "Test 3: doctor still targets the expected endpoint and subcommand"
reset_env
setup_stubs
export CANARY_API_KEY="$SECRET_VALUE"
run_entrypoint > /dev/null
CARGO_ARGV="$(cat "$CARGO_LOG")"
assert_contains "$CARGO_ARGV" "run -q -p canary-cli -- --endpoint http://canary:4000 --json doctor" \
  "invokes the doctor subcommand against the default compose endpoint"

# --- Test 4: endpoint override is honored ---
echo "Test 4: CANARY_DOCTOR_ENDPOINT override is honored"
reset_env
setup_stubs
export CANARY_API_KEY="$SECRET_VALUE"
export CANARY_DOCTOR_ENDPOINT="http://localhost:4000"
run_entrypoint > /dev/null
CARGO_ARGV="$(cat "$CARGO_LOG")"
assert_contains "$CARGO_ARGV" "--endpoint http://localhost:4000" \
  "honors a CANARY_DOCTOR_ENDPOINT override"

# --- Summary ---
echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
