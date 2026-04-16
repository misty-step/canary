#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DR_STATUS="$ROOT/bin/dr-status"
DR_RESTORE_CHECK="$ROOT/bin/dr-restore-check"
BASH_BIN="$(command -v bash)"
MISSING_FLYCTL_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
ORIGINAL_PATH="$PATH"
PASS=0
FAIL=0
TMPDIR_TEST=$(mktemp -d)
trap 'rm -rf "$TMPDIR_TEST"' EXIT

setup_stubs() {
  rm -rf "$TMPDIR_TEST/bin"
  mkdir -p "$TMPDIR_TEST/bin"
  export PATH="$TMPDIR_TEST/bin:$ORIGINAL_PATH"
  export FLYCTL_LOG="$TMPDIR_TEST/flyctl.log"

  cat > "$TMPDIR_TEST/bin/flyctl" <<'STUB'
#!/usr/bin/env bash
{
  printf 'argc=%s\n' "$#"
  i=1
  for arg in "$@"; do
    printf 'arg%s=%s\n' "$i" "$arg"
    i=$((i + 1))
  done
} > "${FLYCTL_LOG:?}"
exit "${FLYCTL_STUB_EXIT:-0}"
STUB
  chmod +x "$TMPDIR_TEST/bin/flyctl"
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

assert_file_matches() {
  local path="$1" pattern="$2" test_name="$3"
  if grep -Eq "$pattern" "$path"; then
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $test_name"
    echo "    Expected $path to match regex: $pattern"
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

assert_equals() {
  local actual="$1" expected="$2" test_name="$3"
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

echo "Test 1: dr-status help"
OUTPUT=$(run_and_capture "$DR_STATUS" --help)
assert_contains "$OUTPUT" "Usage: bin/dr-status" "shows dr-status usage"

echo "Test 2: dr-status uses default app and status command"
setup_stubs
run_and_capture "$DR_STATUS" >/dev/null
assert_file_matches "$FLYCTL_LOG" '^arg[0-9]+=ssh$' "invokes flyctl ssh"
assert_file_matches "$FLYCTL_LOG" '^arg[0-9]+=canary-obs$' "uses default app"
assert_file_matches "$FLYCTL_LOG" \
  "^arg[0-9]+=sh -eu -c 'litestream status -config /etc/litestream\\.yml'\$" \
  "uses remote litestream status command"

echo "Test 3: dr-status honors FLY_APP env default"
setup_stubs
FLY_APP=canary-env run_and_capture "$DR_STATUS" >/dev/null
assert_file_matches "$FLYCTL_LOG" '^arg[0-9]+=canary-env$' "status wrapper uses env app default"

echo "Test 4: dr-status accepts --app override"
setup_stubs
run_and_capture "$DR_STATUS" --app canary-staging >/dev/null
assert_file_matches "$FLYCTL_LOG" '^arg[0-9]+=canary-staging$' "uses overridden app"

echo "Test 5: dr-status rejects unknown arguments"
OUTPUT=$(run_failure "$DR_STATUS" --bogus)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "status wrapper exits non-zero on unknown argument"
assert_contains "$BODY" "Usage: bin/dr-status" "status wrapper prints usage on unknown argument"

echo "Test 6: dr-status rejects --app without a value"
OUTPUT=$(run_failure "$DR_STATUS" --app)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "status wrapper exits non-zero on missing app value"
assert_contains "$BODY" "Usage: bin/dr-status" "status wrapper prints usage on missing app value"

echo "Test 7: dr-status fails cleanly when flyctl is unavailable"
OUTPUT=$(PATH="$MISSING_FLYCTL_PATH" run_failure "$BASH_BIN" "$DR_STATUS")
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "status wrapper exits non-zero when flyctl is missing"
assert_contains "$BODY" "Missing required command: flyctl" "status wrapper names the missing dependency"

echo "Test 8: dr-status propagates flyctl failures"
setup_stubs
export FLYCTL_STUB_EXIT=23
set +e
"$DR_STATUS" >/dev/null 2>&1
STATUS=$?
set -e
unset FLYCTL_STUB_EXIT
assert_equals "$STATUS" "23" "propagates flyctl exit status"

echo "Test 9: dr-restore-check help"
OUTPUT=$(run_and_capture "$DR_RESTORE_CHECK" --help)
assert_contains "$OUTPUT" "Usage: bin/dr-restore-check" "shows dr-restore-check usage"

echo "Test 10: dr-restore-check uses default app and db path"
setup_stubs
run_and_capture "$DR_RESTORE_CHECK" >/dev/null
assert_file_matches "$FLYCTL_LOG" '^arg[0-9]+=canary-obs$' "restore check uses default app"
assert_file_contains "$FLYCTL_LOG" "restore_path=\$(mktemp /tmp/canary-restore.XXXXXX);" \
  "restore check creates a temporary restore path"
assert_file_contains "$FLYCTL_LOG" \
  "litestream restore -if-replica-exists -o \"\$restore_path\" -config /etc/litestream.yml \"\$1\"" \
  "restore check restores the default database path"
assert_file_contains "$FLYCTL_LOG" \
  "if [ ! -s \"\$restore_path\" ]; then echo \"Litestream restore did not materialize a non-empty file at \$restore_path\" >&2; exit 1; fi;" \
  "restore check requires a non-empty restore artifact"
assert_file_matches "$FLYCTL_LOG" \
  "^arg[0-9]+=.* sh /data/canary\\.db\$" \
  "restore check targets the default database path"

echo "Test 11: dr-restore-check uses the remote DB default even when local CANARY_DB_PATH is set"
setup_stubs
FLY_APP=canary-env CANARY_DB_PATH=/data/env.db run_and_capture "$DR_RESTORE_CHECK" >/dev/null
assert_file_matches "$FLYCTL_LOG" '^arg[0-9]+=canary-env$' "restore check uses env app default"
assert_file_matches "$FLYCTL_LOG" "^arg[0-9]+=.* sh /data/canary\\.db\$" "restore check ignores local db env default"

echo "Test 12: dr-restore-check accepts overrides"
setup_stubs
run_and_capture "$DR_RESTORE_CHECK" --app canary-staging --db-path /data/restore.db >/dev/null
assert_file_matches "$FLYCTL_LOG" '^arg[0-9]+=canary-staging$' "restore check uses overridden app"
assert_file_matches "$FLYCTL_LOG" \
  "^arg[0-9]+=.* sh /data/restore\\.db\$" \
  "restore check uses overridden db path"

echo "Test 13: dr-restore-check rejects unknown arguments"
OUTPUT=$(run_failure "$DR_RESTORE_CHECK" --bogus)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "restore check exits non-zero on unknown argument"
assert_contains "$BODY" "Usage: bin/dr-restore-check" "restore check prints usage on unknown argument"

echo "Test 14: dr-restore-check safely quotes apostrophes in db paths"
setup_stubs
run_and_capture "$DR_RESTORE_CHECK" --db-path "/data/o'brien.db" >/dev/null
assert_file_matches "$FLYCTL_LOG" \
  "^arg[0-9]+=.* sh /data/o\\\\'brien\\.db\$" \
  "restore check shell-quotes db paths"

echo "Test 15: dr-restore-check safely quotes spaces in db paths"
setup_stubs
run_and_capture "$DR_RESTORE_CHECK" --db-path "/data/restore copy.db" >/dev/null
assert_file_matches "$FLYCTL_LOG" \
  '^arg[0-9]+=.* sh /data/restore\\ copy\.db$' \
  "restore check shell-quotes paths with spaces"

echo "Test 16: dr-restore-check rejects --db-path without a value"
OUTPUT=$(run_failure "$DR_RESTORE_CHECK" --db-path)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "restore check exits non-zero on missing db path"
assert_contains "$BODY" "Usage: bin/dr-restore-check" "restore check prints usage on missing db path"

echo "Test 17: dr-restore-check rejects --app without a value"
OUTPUT=$(run_failure "$DR_RESTORE_CHECK" --app)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "restore check exits non-zero on missing app value"
assert_contains "$BODY" "Usage: bin/dr-restore-check" "restore check prints usage on missing app value"

echo "Test 18: dr-restore-check fails cleanly when flyctl is unavailable"
OUTPUT=$(PATH="$MISSING_FLYCTL_PATH" run_failure "$BASH_BIN" "$DR_RESTORE_CHECK")
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "restore check exits non-zero when flyctl is missing"
assert_contains "$BODY" "Missing required command: flyctl" "restore check names the missing dependency"

echo "Test 19: dr-restore-check propagates flyctl failures"
setup_stubs
export FLYCTL_STUB_EXIT=29
set +e
"$DR_RESTORE_CHECK" >/dev/null 2>&1
STATUS=$?
set -e
unset FLYCTL_STUB_EXIT
assert_equals "$STATUS" "29" "restore check propagates flyctl exit status"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
