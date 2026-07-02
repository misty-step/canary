#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DOGFOOD_INVENTORY="$ROOT/bin/dogfood-inventory"
PASS=0
FAIL=0
TMPDIR_TEST="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_TEST"' EXIT

write_manifest() {
  local path="$1"
  cat >"$path" <<'JSON'
{
  "schema_version": 1,
  "services": [
    {
      "service": "alpha",
      "state": "active",
      "platform": "vercel",
      "platform_project": "alpha",
      "production_url": "https://alpha.example",
      "repo_path": null,
      "health_url": "https://alpha.example/api/health",
      "monitor_mode": "http",
      "ingest_status": "verified",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "No current blocker.",
      "owner": "example-org",
      "next_action": "Keep enrolled."
    },
    {
      "service": "canary-self",
      "state": "active",
      "platform": "fly",
      "platform_project": "canary-example",
      "production_url": "https://canary.example",
      "repo_path": null,
      "health_url": "https://canary.example/healthz",
      "monitor_mode": "http",
      "ingest_status": "verified",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "Self target is enrolled.",
      "owner": "example-org",
      "next_action": "Keep enrolled."
    },
    {
      "service": "bravo",
      "state": "pending",
      "platform": "vercel",
      "platform_project": "bravo",
      "production_url": "https://bravo.example",
      "repo_path": null,
      "health_url": "https://bravo.example/api/health",
      "monitor_mode": "http",
      "ingest_status": "partial",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "Target not enrolled yet.",
      "owner": "example-org",
      "next_action": "Enroll the target."
    },
    {
      "service": "charlie",
      "state": "blocked",
      "platform": "vercel",
      "platform_project": "charlie",
      "production_url": "https://charlie.example",
      "repo_path": null,
      "health_url": null,
      "monitor_mode": "none",
      "ingest_status": "blocked",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "No health route exists.",
      "owner": "example-org",
      "next_action": "Add a health route."
    },
    {
      "service": "delta",
      "state": "ignored",
      "platform": "vercel",
      "platform_project": "delta",
      "production_url": "https://delta.example",
      "repo_path": null,
      "health_url": null,
      "monitor_mode": "none",
      "ingest_status": "not_applicable",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "Retired.",
      "owner": "example-org",
      "next_action": "No action."
    }
  ]
}
JSON
}

write_invalid_manifest() {
  local path="$1"
  cat >"$path" <<'JSON'
{
  "schema_version": 1,
  "services": [
    {
      "service": "alpha",
      "state": "active",
      "platform": "vercel",
      "platform_project": "alpha",
      "production_url": "https://alpha.example",
      "repo_path": null,
      "health_url": "https://alpha.example/api/health",
      "last_checked_at": "2026-06-11T00:00:00Z",
      "failure_mode": "No current blocker.",
      "owner": "example-org",
      "next_action": "Keep enrolled."
    }
  ]
}
JSON
}

write_stale_manifest() {
  local path="$1"
  cat >"$path" <<'JSON'
{
  "schema_version": 1,
  "services": [
    {
      "service": "stale",
      "state": "active",
      "platform": "vercel",
      "platform_project": "stale",
      "production_url": "https://stale.example",
      "repo_path": null,
      "health_url": "https://stale.example/api/health",
      "monitor_mode": "http",
      "ingest_status": "verified",
      "last_checked_at": "2026-06-15T00:00:00Z",
      "failure_mode": "Old evidence.",
      "owner": "example-org",
      "next_action": "Finish ticket 038."
    }
  ]
}
JSON
}

write_vercel_projects() {
  local path="$1"
  cat >"$path" <<'JSON'
{
  "projects": [
    {"name": "alpha", "targets": {"production": {"alias": ["alpha.example"]}}},
    {"name": "bravo", "targets": {"production": {"alias": ["bravo.example"]}}}
  ]
}
JSON
}

write_empty_vercel_projects() {
  local path="$1"
  cat >"$path" <<'JSON'
{"projects":[]}
JSON
}

write_vercel_projects_with_rogue() {
  local path="$1"
  cat >"$path" <<'JSON'
{
  "projects": [
    {"name": "alpha", "url": "https://alpha.example"},
    {"name": "bravo", "url": "https://bravo.example"},
    {"name": "rogue", "url": "https://rogue.example"}
  ]
}
JSON
}

write_fly_apps() {
  local path="$1"
  cat >"$path" <<'JSON'
[
  {"Name": "canary-example", "Organization": {"Slug": "example-org"}, "Hostname": "canary.example"}
]
JSON
}

write_local_links() {
  local root="$1"
  mkdir -p "$root/alpha/.vercel"
  cat >"$root/alpha/.vercel/project.json" <<'JSON'
{"projectId":"prj_alpha","projectName":"alpha"}
JSON
}

write_receipt() {
  local root="$1" service="$2" status="$3" target_id="$4"
  mkdir -p "$root/$service/.canary"
  cat >"$root/$service/.canary/integration.json" <<JSON
{
  "schema_version": 1,
  "service": "$service",
  "environment": "production",
  "canary_endpoint": "https://canary.example",
  "health_url": "https://$service.example/api/health",
  "target_id": $target_id,
  "monitor_ids": [],
  "webhook_ids": [],
  "api_key_id": "KEY-1",
  "verification_status": "$status",
  "env_names": ["CANARY_ENDPOINT", "CANARY_API_KEY"],
  "verification_commands": ["bin/canary integrate status . --service $service --json"],
  "last_verified_at": "1781222400"
}
JSON
}

setup_stubbed_collectors() {
  local bin_dir="$1"
  mkdir -p "$bin_dir"
  cat >"$bin_dir/vercel" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

scope=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --scope)
      scope="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

case "$scope" in
  example-team)
    cat <<'JSON'
{"projects":[{"name":"alpha","url":"https://alpha.example"},{"name":"bravo","url":"https://bravo.example"}]}
JSON
    ;;
  example-admin)
    cat <<'JSON'
{"projects":[]}
JSON
    ;;
  *)
    printf 'unexpected scope: %s\n' "$scope" >&2
    exit 99
    ;;
esac
STUB
  chmod +x "$bin_dir/vercel"

  cat >"$bin_dir/flyctl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

cat <<'JSON'
[{"Name":"canary-example","Organization":{"Slug":"example-org"},"Hostname":"canary.example"}]
JSON
STUB
  chmod +x "$bin_dir/flyctl"
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
    echo "    Filter: $filter"
    echo "    Expected: $expected"
    echo "    Got: $actual"
    FAIL=$((FAIL + 1))
  fi
}

MANIFEST="$TMPDIR_TEST/manifest.json"
INVALID_MANIFEST="$TMPDIR_TEST/invalid-manifest.json"
STALE_MANIFEST="$TMPDIR_TEST/stale-manifest.json"
VERCEL_PROJECTS="$TMPDIR_TEST/vercel.json"
VERCEL_ROGUE="$TMPDIR_TEST/vercel-rogue.json"
VERCEL_EMPTY="$TMPDIR_TEST/vercel-empty.json"
FLY_APPS="$TMPDIR_TEST/fly.json"
LOCAL_ROOT="$TMPDIR_TEST/workspace"

write_manifest "$MANIFEST"
write_invalid_manifest "$INVALID_MANIFEST"
write_stale_manifest "$STALE_MANIFEST"
write_vercel_projects "$VERCEL_PROJECTS"
write_vercel_projects_with_rogue "$VERCEL_ROGUE"
write_empty_vercel_projects "$VERCEL_EMPTY"
write_fly_apps "$FLY_APPS"
write_local_links "$LOCAL_ROOT"
write_receipt "$LOCAL_ROOT" "bravo" "verified" '"TGT-2"'
write_receipt "$LOCAL_ROOT" "charlie" "planned" 'null'

echo "Test 1: dogfood-inventory help"
OUTPUT=$(run_and_capture "$DOGFOOD_INVENTORY" --help)
assert_contains "$OUTPUT" "Usage: bin/dogfood-inventory" "shows dogfood-inventory usage"
assert_contains "$OUTPUT" "--vercel-projects" "documents Vercel fixtures"
assert_contains "$OUTPUT" ".canary/dogfood/owned_services.json" "documents instance-local registry"

echo "Test 1b: dogfood-inventory fails cleanly without local registry"
OUTPUT=$(run_failure "$DOGFOOD_INVENTORY" --fly-apps "$FLY_APPS" --local-root "$LOCAL_ROOT" --now 2026-06-12T00:00:00Z --json)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "2" "missing default registry exits non-zero"
assert_contains "$BODY" ".canary/dogfood/owned_services.json" "missing registry points to instance-local path"

echo "Test 2: dogfood-inventory emits coverage json"
OUTPUT=$(run_and_capture "$DOGFOOD_INVENTORY" --manifest "$MANIFEST" --vercel-projects "example-team=$VERCEL_PROJECTS" --vercel-projects "example-admin=$VERCEL_EMPTY" --fly-apps "$FLY_APPS" --local-root "$LOCAL_ROOT" --requested alpha,bravo,canary-self --now 2026-06-12T00:00:00Z --json --strict)
assert_json_equals "$OUTPUT" ".summary.covered" "2" "json counts covered services"
assert_json_equals "$OUTPUT" ".summary.partial" "1" "json counts partial services"
assert_json_equals "$OUTPUT" ".summary.blocked" "1" "json counts blocked services"
assert_json_equals "$OUTPUT" ".summary.ignored" "1" "json counts ignored services"
assert_json_equals "$OUTPUT" ".summary.strict_failures" "0" "strict fixture has no failures"
assert_json_equals "$OUTPUT" ".surfaces[] | select(.service == \"alpha\") | .local_link_seen" "true" "local Vercel link is joined to registry service"
assert_json_equals "$OUTPUT" ".surfaces[] | select(.service == \"bravo\") | .receipt_seen" "true" "verified receipt is joined to registry service"
assert_json_equals "$OUTPUT" ".surfaces[] | select(.service == \"charlie\") | .receipt_seen" "true" "planned receipt is visible in inventory"
assert_json_equals "$OUTPUT" ".surfaces[] | select(.service == \"charlie\") | .coverage" "blocked" "planned receipt does not create coverage"

echo "Test 3: dogfood-inventory strict fails on unregistered deployment"
OUTPUT=$(run_failure "$DOGFOOD_INVENTORY" --manifest "$MANIFEST" --vercel-projects "example-team=$VERCEL_ROGUE" --vercel-projects "example-admin=$VERCEL_EMPTY" --fly-apps "$FLY_APPS" --local-root "$LOCAL_ROOT" --requested alpha,bravo,canary-self --now 2026-06-12T00:00:00Z --strict)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "strict unregistered deployment exits non-zero"
assert_contains "$BODY" "unregistered_deployment" "strict output names unregistered deployment"
assert_contains "$BODY" "rogue" "strict output names rogue project"

echo "Test 4: dogfood-inventory strict fails on missing requested service"
OUTPUT=$(run_failure "$DOGFOOD_INVENTORY" --manifest "$MANIFEST" --vercel-projects "example-team=$VERCEL_PROJECTS" --vercel-projects "example-admin=$VERCEL_EMPTY" --fly-apps "$FLY_APPS" --local-root "$LOCAL_ROOT" --requested alpha,missing --now 2026-06-12T00:00:00Z --strict)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "missing requested registry entry exits non-zero"
assert_contains "$BODY" "missing_requested_registry_entry" "strict output names missing requested service"

echo "Test 5: dogfood-inventory rejects invalid registry shape"
OUTPUT=$(run_failure "$DOGFOOD_INVENTORY" --manifest "$INVALID_MANIFEST" --vercel-projects "example-team=$VERCEL_PROJECTS" --vercel-projects "example-admin=$VERCEL_EMPTY" --fly-apps "$FLY_APPS" --local-root "$LOCAL_ROOT")
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "2" "invalid registry exits non-zero"
assert_contains "$BODY" "invalid dogfood registry shape" "invalid registry explains schema failure"

echo "Test 6: dogfood-inventory strict fails stale evidence and singular completed-ticket next actions"
OUTPUT=$(run_failure "$DOGFOOD_INVENTORY" --manifest "$STALE_MANIFEST" --vercel-projects "example-team=$VERCEL_EMPTY" --vercel-projects "example-admin=$VERCEL_EMPTY" --fly-apps "$FLY_APPS" --local-root "$LOCAL_ROOT" --requested stale --now 2026-06-14T00:00:00Z --max-evidence-age-hours 24 --strict)
STATUS=$(printf '%s' "$OUTPUT" | head -n 1)
BODY=$(printf '%s' "$OUTPUT" | tail -n +2)
assert_exit_code "$STATUS" "1" "stale registry evidence exits non-zero"
assert_contains "$BODY" "stale_registry_evidence" "strict output names stale evidence"
assert_contains "$BODY" "completed_ticket_next_action" "strict output names completed-ticket next action"

echo "Test 7: dogfood-inventory supports no-fixture collector path"
STUB_BIN="$TMPDIR_TEST/bin"
setup_stubbed_collectors "$STUB_BIN"
OUTPUT=$(PATH="$STUB_BIN:$PATH" run_and_capture "$DOGFOOD_INVENTORY" --manifest "$MANIFEST" --vercel-scope example-team --vercel-scope example-admin --local-root "$LOCAL_ROOT" --requested alpha,bravo,canary-self --now 2026-06-12T00:00:00Z --json --strict)
assert_json_equals "$OUTPUT" ".summary.strict_failures" "0" "stubbed live collectors satisfy strict mode"
assert_json_equals "$OUTPUT" ".collector_errors | length" "0" "stubbed live collectors avoid collector errors"
assert_json_equals "$OUTPUT" ".surfaces[] | select(.service == \"canary-self\") | .deployment_seen" "true" "stubbed Fly collector joins active self service"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
