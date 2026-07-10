#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DR_STATUS="$ROOT/bin/dr-status"
DR_RESTORE_CHECK="$ROOT/bin/dr-restore-check"
BASH_BIN="$(command -v bash)"
ORIGINAL_PATH="$PATH"
PASS=0
FAIL=0
TMPDIR_TEST=$(mktemp -d)
trap 'rm -rf "$TMPDIR_TEST"' EXIT
MISSING_SSH_PATH="$TMPDIR_TEST/no-ssh"
mkdir -p "$MISSING_SSH_PATH"
ln -s "$(command -v dirname)" "$MISSING_SSH_PATH/dirname"

setup_stubs() {
  rm -rf "$TMPDIR_TEST/bin"
  mkdir -p "$TMPDIR_TEST/bin"
  export PATH="$TMPDIR_TEST/bin:$ORIGINAL_PATH"
  export SSH_LOG="$TMPDIR_TEST/ssh.log"
  export SSH_STDIN_LOG="$TMPDIR_TEST/ssh.stdin.log"

  cat > "$TMPDIR_TEST/bin/ssh" <<'STUB'
#!/usr/bin/env bash
{
  printf 'argc=%s\n' "$#"
  i=1
  for arg in "$@"; do
    printf 'arg%s=%s\n' "$i" "$arg"
    i=$((i + 1))
  done
} > "${SSH_LOG:?}"
cat > "${SSH_STDIN_LOG:?}"
exit "${SSH_STUB_EXIT:-0}"
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

echo "Test 2: dr-status requires an explicit host"
OUTPUT=$(run_failure "$DR_STATUS")
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "status wrapper exits non-zero without host"
assert_contains "$BODY" "Missing Canary SSH host: pass --host or set CANARY_SSH_HOST" "status wrapper names host configuration"

echo "Test 3: dr-status honors CANARY_SSH_HOST and default container"
setup_stubs
CANARY_SSH_HOST=canary-host run_and_capture "$DR_STATUS" >/dev/null
assert_file_contains "$SSH_LOG" "arg1=canary-host" "uses configured SSH host"
assert_file_contains "$SSH_LOG" "arg2=sudo" "uses host privilege boundary"
assert_file_contains "$SSH_LOG" "arg3=docker" "uses Docker runtime"
assert_file_contains "$SSH_LOG" "arg4=exec" "executes inside running container"
assert_file_contains "$SSH_LOG" "arg6=canary" "uses canonical container name"
assert_file_contains "$SSH_STDIN_LOG" "litestream status -config /etc/litestream.yml" "runs Litestream status in the container"

echo "Test 4: dr-status accepts host and container overrides"
setup_stubs
run_and_capture "$DR_STATUS" --host canary-staging --container canary-staging >/dev/null
assert_file_contains "$SSH_LOG" "arg1=canary-staging" "uses overridden host"
assert_file_contains "$SSH_LOG" "arg6=canary-staging" "uses overridden container"

echo "Test 5: dr-status rejects unsafe container names"
OUTPUT=$(run_failure "$DR_STATUS" --host canary-host --container 'canary;id')
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "status wrapper rejects unsafe container"
assert_contains "$BODY" "Invalid Canary container name" "status wrapper names unsafe container"

echo "Test 6: dr-status rejects unknown arguments"
OUTPUT=$(run_failure "$DR_STATUS" --bogus)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "status wrapper exits non-zero on unknown argument"
assert_contains "$BODY" "Usage: bin/dr-status" "status wrapper prints usage on unknown argument"

echo "Test 6b: dr-status rejects SSH option injection"
OUTPUT=$(run_failure "$DR_STATUS" --host '-oProxyCommand=bad')
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "status wrapper rejects SSH option host"
assert_contains "$BODY" "Invalid Canary SSH host" "status wrapper names unsafe host"

echo "Test 7: dr-status fails cleanly when ssh is unavailable"
OUTPUT=$(PATH="$MISSING_SSH_PATH" run_failure "$BASH_BIN" "$DR_STATUS" --host canary-host)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "status wrapper exits non-zero when ssh is missing"
assert_contains "$BODY" "Missing required command: ssh" "status wrapper names the missing dependency"

echo "Test 8: dr-status propagates ssh failures"
setup_stubs
export SSH_STUB_EXIT=23
set +e
"$DR_STATUS" --host canary-host >/dev/null 2>&1
STATUS=$?
set -e
unset SSH_STUB_EXIT
assert_equals "$STATUS" "23" "propagates ssh exit status"

echo "Test 9: dr-restore-check help"
OUTPUT=$(run_and_capture "$DR_RESTORE_CHECK" --help)
assert_contains "$OUTPUT" "Usage: bin/dr-restore-check" "shows dr-restore-check usage"

echo "Test 10: dr-restore-check requires an explicit host"
OUTPUT=$(run_failure "$DR_RESTORE_CHECK")
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "restore check exits non-zero without host"
assert_contains "$BODY" "Missing Canary SSH host: pass --host or set CANARY_SSH_HOST" "restore check names host configuration"

echo "Test 11: dr-restore-check uses configured host, container, and DB path"
setup_stubs
CANARY_SSH_HOST=canary-host run_and_capture "$DR_RESTORE_CHECK" >/dev/null
assert_file_contains "$SSH_LOG" "arg1=canary-host" "restore check uses configured host"
assert_file_contains "$SSH_LOG" "arg6=canary" "restore check uses canonical container"
assert_file_contains "$SSH_STDIN_LOG" 'restore_path=$(mktemp /tmp/canary-restore.XXXXXX)' "restore check creates a temporary restore path"
assert_file_contains "$SSH_STDIN_LOG" 'db_path=/data/canary.db' "restore check targets the canonical database path"
assert_file_contains "$SSH_STDIN_LOG" 'litestream restore -if-replica-exists -o "$restore_path" -config /etc/litestream.yml "$db_path"' "restore check restores through mounted Litestream configuration"
assert_file_contains "$SSH_STDIN_LOG" 'if [ ! -s "$restore_path" ]' "restore check requires a non-empty restore artifact"

echo "Test 12: dr-restore-check accepts safe overrides"
setup_stubs
run_and_capture "$DR_RESTORE_CHECK" --host canary-staging --container canary-staging --db-path /data/restore.db >/dev/null
assert_file_contains "$SSH_LOG" "arg1=canary-staging" "restore check uses overridden host"
assert_file_contains "$SSH_LOG" "arg6=canary-staging" "restore check uses overridden container"
assert_file_contains "$SSH_STDIN_LOG" "db_path=/data/restore.db" "restore check uses overridden db path"

echo "Test 13: dr-restore-check rejects unsafe DB paths"
OUTPUT=$(run_failure "$DR_RESTORE_CHECK" --host canary-host --db-path '/data/restore copy.db')
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "restore check rejects unsafe DB path"
assert_contains "$BODY" "Invalid Canary database path" "restore check names unsafe DB path"

echo "Test 14: dr-restore-check rejects unknown arguments"
OUTPUT=$(run_failure "$DR_RESTORE_CHECK" --bogus)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "restore check exits non-zero on unknown argument"
assert_contains "$BODY" "Usage: bin/dr-restore-check" "restore check prints usage on unknown argument"

echo "Test 15: dr-restore-check fails cleanly when ssh is unavailable"
OUTPUT=$(PATH="$MISSING_SSH_PATH" run_failure "$BASH_BIN" "$DR_RESTORE_CHECK" --host canary-host)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "restore check exits non-zero when ssh is missing"
assert_contains "$BODY" "Missing required command: ssh" "restore check names the missing dependency"

echo "Test 16: dr-restore-check propagates ssh failures"
setup_stubs
export SSH_STUB_EXIT=29
set +e
"$DR_RESTORE_CHECK" --host canary-host >/dev/null 2>&1
STATUS=$?
set -e
unset SSH_STUB_EXIT
assert_equals "$STATUS" "29" "restore check propagates ssh exit status"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
