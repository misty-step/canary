#!/bin/bash
# Tests for entrypoint.sh Litestream env validation.
# Runs the entrypoint in a subshell with stubbed exec/litestream to capture warnings.
set -e

ENTRYPOINT="$(cd "$(dirname "$0")/../.." && pwd)/bin/entrypoint.sh"
PASS=0
FAIL=0
TMPDIR_TEST=$(mktemp -d)
trap 'rm -rf "$TMPDIR_TEST"' EXIT

reset_env() {
  unset LITESTREAM_S3_BUCKET
  unset LITESTREAM_ACCESS_KEY_ID
  unset LITESTREAM_SECRET_ACCESS_KEY
  unset LITESTREAM_S3_REGION
}

setup_stubs() {
  export PATH="$TMPDIR_TEST/bin:$PATH"
  mkdir -p "$TMPDIR_TEST/bin"
  cat > "$TMPDIR_TEST/bin/litestream" << 'STUB'
#!/bin/bash
exit "${LITESTREAM_STUB_EXIT:-0}"
STUB
  chmod +x "$TMPDIR_TEST/bin/litestream"
  export CANARY_DB_PATH="$TMPDIR_TEST/canary.db"
  touch "$CANARY_DB_PATH"
}

run_entrypoint() {
  # Override exec to echo what would run, then exit
  bash -c "exec() { echo \"EXEC:\$*\"; exit 0; }; source '$ENTRYPOINT'" 2>&1
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

assert_not_contains() {
  local output="$1" unexpected="$2" test_name="$3"
  if echo "$output" | grep -qF "$unexpected"; then
    echo "  FAIL: $test_name"
    echo "    Expected NOT to contain: $unexpected"
    echo "    Got: $output"
    FAIL=$((FAIL + 1))
  else
    echo "  PASS: $test_name"
    PASS=$((PASS + 1))
  fi
}

# --- Test 1: No bucket → warn about missing replication ---
echo "Test 1: LITESTREAM_S3_BUCKET unset"
reset_env
setup_stubs
OUTPUT=$(run_entrypoint)
assert_contains "$OUTPUT" "Litestream replication NOT configured — running without backups" \
  "warns about missing replication"

# --- Test 2: Bucket set, creds missing → warn about missing vars ---
echo "Test 2: LITESTREAM_S3_BUCKET set, ACCESS_KEY_ID missing"
reset_env
setup_stubs
export LITESTREAM_S3_BUCKET="my-bucket"
export LITESTREAM_SECRET_ACCESS_KEY="secret"
export LITESTREAM_S3_REGION="us-east-1"
OUTPUT=$(run_entrypoint)
assert_contains "$OUTPUT" "LITESTREAM_ACCESS_KEY_ID" \
  "identifies missing ACCESS_KEY_ID"
assert_not_contains "$OUTPUT" "NOT configured" \
  "does not warn about unconfigured replication"

# --- Test 3: All vars set → no warnings ---
echo "Test 3: All Litestream vars set"
reset_env
setup_stubs
export LITESTREAM_S3_BUCKET="my-bucket"
export LITESTREAM_ACCESS_KEY_ID="key"
export LITESTREAM_SECRET_ACCESS_KEY="secret"
export LITESTREAM_S3_REGION="us-east-1"
OUTPUT=$(run_entrypoint)
assert_not_contains "$OUTPUT" "WARNING" \
  "no warnings when fully configured"

# --- Test 4: Bucket set, multiple creds missing ---
echo "Test 4: LITESTREAM_S3_BUCKET set, multiple vars missing"
reset_env
setup_stubs
export LITESTREAM_S3_BUCKET="my-bucket"
OUTPUT=$(run_entrypoint)
assert_contains "$OUTPUT" "LITESTREAM_ACCESS_KEY_ID" \
  "identifies missing ACCESS_KEY_ID"
assert_contains "$OUTPUT" "LITESTREAM_SECRET_ACCESS_KEY" \
  "identifies missing SECRET_ACCESS_KEY"
assert_contains "$OUTPUT" "LITESTREAM_S3_REGION" \
  "identifies missing S3_REGION"

# --- Test 5: Missing creds → app starts directly, not via litestream ---
echo "Test 5: Missing creds do not block startup"
reset_env
setup_stubs
export LITESTREAM_S3_BUCKET="my-bucket"
OUTPUT=$(run_entrypoint)
assert_contains "$OUTPUT" "EXEC:/app/bin/canary start" \
  "starts app directly when creds missing"
assert_not_contains "$OUTPUT" "litestream replicate" \
  "does not run litestream replicate when creds missing"

# --- Test 6: All vars set → starts via litestream ---
echo "Test 6: Full config starts via litestream"
reset_env
setup_stubs
export LITESTREAM_S3_BUCKET="my-bucket"
export LITESTREAM_ACCESS_KEY_ID="key"
export LITESTREAM_SECRET_ACCESS_KEY="secret"
export LITESTREAM_S3_REGION="us-east-1"
OUTPUT=$(run_entrypoint)
assert_contains "$OUTPUT" "EXEC:litestream replicate" \
  "starts via litestream when fully configured"

# --- Summary ---
echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
