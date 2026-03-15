#!/bin/bash
# Tests for entrypoint.sh Litestream env validation.
# Runs the entrypoint in a subshell with stubbed exec/litestream to capture warnings.
set -e

ENTRYPOINT="$(cd "$(dirname "$0")/../.." && pwd)/bin/entrypoint.sh"
PASS=0
FAIL=0
TMPDIR_TEST=$(mktemp -d)
trap 'rm -rf "$TMPDIR_TEST"' EXIT

# Stub: override exec and litestream so entrypoint doesn't actually launch anything
setup_stubs() {
  export PATH="$TMPDIR_TEST/bin:$PATH"
  mkdir -p "$TMPDIR_TEST/bin"
  # exec replacement — just exit cleanly
  cat > "$TMPDIR_TEST/bin/litestream" << 'STUB'
#!/bin/bash
exit 0
STUB
  chmod +x "$TMPDIR_TEST/bin/litestream"
  # Fake DB path so restore is skipped
  export CANARY_DB_PATH="$TMPDIR_TEST/canary.db"
  touch "$CANARY_DB_PATH"
}

run_entrypoint() {
  # Run in subshell, capture stderr, override exec to just exit
  bash -c "exec() { exit 0; }; source '$ENTRYPOINT'" 2>&1
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
setup_stubs
unset LITESTREAM_S3_BUCKET
unset LITESTREAM_ACCESS_KEY_ID
unset LITESTREAM_SECRET_ACCESS_KEY
unset LITESTREAM_S3_REGION
OUTPUT=$(run_entrypoint)
assert_contains "$OUTPUT" "Litestream replication NOT configured — running without backups" \
  "warns about missing replication"

# --- Test 2: Bucket set, creds missing → warn about missing vars ---
echo "Test 2: LITESTREAM_S3_BUCKET set, ACCESS_KEY_ID missing"
setup_stubs
export LITESTREAM_S3_BUCKET="my-bucket"
export LITESTREAM_SECRET_ACCESS_KEY="secret"
export LITESTREAM_S3_REGION="us-east-1"
unset LITESTREAM_ACCESS_KEY_ID
OUTPUT=$(run_entrypoint)
assert_contains "$OUTPUT" "LITESTREAM_ACCESS_KEY_ID" \
  "identifies missing ACCESS_KEY_ID"
assert_not_contains "$OUTPUT" "NOT configured" \
  "does not warn about unconfigured replication"

# --- Test 3: All vars set → no warnings ---
echo "Test 3: All Litestream vars set"
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
setup_stubs
export LITESTREAM_S3_BUCKET="my-bucket"
unset LITESTREAM_ACCESS_KEY_ID
unset LITESTREAM_SECRET_ACCESS_KEY
unset LITESTREAM_S3_REGION
OUTPUT=$(run_entrypoint)
assert_contains "$OUTPUT" "LITESTREAM_ACCESS_KEY_ID" \
  "identifies missing ACCESS_KEY_ID"
assert_contains "$OUTPUT" "LITESTREAM_SECRET_ACCESS_KEY" \
  "identifies missing SECRET_ACCESS_KEY"
assert_contains "$OUTPUT" "LITESTREAM_S3_REGION" \
  "identifies missing S3_REGION"

# --- Summary ---
echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
