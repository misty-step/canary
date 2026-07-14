#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/bin/canary-recovery"
PASS=0
FAIL=0
TEST_TMP="$(mktemp -d)"
ORIGINAL_PATH="$PATH"
trap 'rm -rf "$TEST_TMP"' EXIT

assert_contains() {
  local actual="$1" expected="$2" name="$3"
  if grep -qF -- "$expected" <<<"$actual"; then
    printf '  PASS: %s\n' "$name"
    PASS=$((PASS + 1))
  else
    printf '  FAIL: %s\n    expected: %s\n    actual: %s\n' "$name" "$expected" "$actual"
    FAIL=$((FAIL + 1))
  fi
}

assert_jq() {
  local actual="$1" filter="$2" name="$3"
  if jq -e "$filter" >/dev/null <<<"$actual"; then
    printf '  PASS: %s\n' "$name"
    PASS=$((PASS + 1))
  else
    printf '  FAIL: %s\n    filter: %s\n    actual: %s\n' "$name" "$filter" "$actual"
    FAIL=$((FAIL + 1))
  fi
}

run_failure() {
  set +e
  local output
  output="$("$@" 2>&1)"
  local status=$?
  set -e
  printf '%s\n%s' "$status" "$output"
}

mkdir -p "$TEST_TMP/bin"
CONFIG="$TEST_TMP/litestream.yml"
printf 'dbs: []\n' > "$CONFIG"

cat > "$TEST_TMP/bin/litestream" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${LITESTREAM_LOG:?}"
if [[ "${1:-}" == "restore" ]]; then
  while (($#)); do
    if [[ "$1" == "-o" ]]; then
      printf 'restored sqlite bytes' > "$2"
      break
    fi
    shift
  done
fi
STUB

cat > "$TEST_TMP/bin/canary-server" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${SERVER_LOG:?}"
case "${1:-}" in
  version)
    printf '%s\n' '{"schema":"canary.runtime-version.v1","version":"test","database_schema_version":7}'
    ;;
  migrate)
    printf '%s\n' '{"schema":"canary.data-verification.v1","schema_version":7,"expected_schema_version":7,"schema_current":true,"integrity_check":"ok","foreign_key_violations":0,"table_counts":{"errors":1}}'
    ;;
  *) exit 1 ;;
esac
STUB
chmod +x "$TEST_TMP/bin/litestream" "$TEST_TMP/bin/canary-server"
export PATH="$TEST_TMP/bin:$ORIGINAL_PATH"
export LITESTREAM_LOG="$TEST_TMP/litestream.log"
export SERVER_LOG="$TEST_TMP/server.log"

echo "Test 1: help exposes only provider-neutral inputs"
OUTPUT="$($SCRIPT --help)"
assert_contains "$OUTPUT" "--config <path>" "documents caller-supplied backup config"
assert_contains "$OUTPUT" "--database <path>" "documents caller-supplied database identity"
if grep -Eqi 'ssh|container|provider|volume|port|mount' <<<"$OUTPUT"; then
  echo "  FAIL: help contains deployment topology"
  FAIL=$((FAIL + 1))
else
  echo "  PASS: help contains no deployment topology"
  PASS=$((PASS + 1))
fi

echo "Test 2: status passes only the supplied Litestream config"
: > "$LITESTREAM_LOG"
$SCRIPT status --config "$CONFIG"
assert_contains "$(cat "$LITESTREAM_LOG")" "status -config $CONFIG" "runs generic status command"

echo "Test 3: restore-check restores, migrates a copy, and emits bounded evidence"
: > "$LITESTREAM_LOG"
: > "$SERVER_LOG"
OUTPUT="$($SCRIPT restore-check --config "$CONFIG" --database /arbitrary/canary.db --server-bin "$TEST_TMP/bin/canary-server")"
assert_jq "$OUTPUT" '.schema == "canary.recovery-check.v1"' "emits stable recovery receipt schema"
assert_jq "$OUTPUT" '.restored_bytes > 0' "records non-empty restore"
assert_jq "$OUTPUT" '.runtime.schema == "canary.runtime-version.v1"' "records runtime version contract"
assert_jq "$OUTPUT" '.migration_and_data.schema_current == true and .migration_and_data.integrity_check == "ok"' "records migration and data verification"
assert_contains "$(cat "$LITESTREAM_LOG")" "restore -if-replica-exists" "uses non-destructive Litestream restore"
assert_contains "$(cat "$SERVER_LOG")" "version" "reads runtime identity"
assert_contains "$(cat "$SERVER_LOG")" "migrate --database" "migrates only the disposable copy"

echo "Test 4: incomplete product inputs fail closed"
OUTPUT="$(run_failure "$SCRIPT" restore-check --config "$CONFIG" --server-bin "$TEST_TMP/bin/canary-server")"
assert_contains "$OUTPUT" "database path required" "missing database is explicit"
OUTPUT="$(run_failure "$SCRIPT" status --config "$TEST_TMP/missing.yml")"
assert_contains "$OUTPUT" "Litestream config not found" "missing config is explicit"

echo "Test 5: compatibility entrypoints route to the portable contract"
: > "$LITESTREAM_LOG"
$ROOT/bin/dr-status --config "$CONFIG"
assert_contains "$(cat "$LITESTREAM_LOG")" "status -config $CONFIG" "dr-status is a provider-neutral alias"

printf '\nResults: %s passed, %s failed\n' "$PASS" "$FAIL"
[[ "$FAIL" == 0 ]]
