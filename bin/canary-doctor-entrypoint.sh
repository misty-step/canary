#!/usr/bin/env bash
set -euo pipefail

# Entrypoint for the docker-compose.yml `canary-doctor` service. Never pass
# CANARY_API_KEY as a literal argv value to cargo/canary-cli — canary-cli
# already reads it from its inherited process environment
# (crates/canary-cli/src/lib.rs Config::resolve_with_mode). Passing it via
# --api-key instead would put the raw key in this container's process
# argv, visible via `docker top`/`ps`/`/proc/<pid>/cmdline`.
if [ -z "${CANARY_API_KEY:-}" ]; then
  echo "set CANARY_API_KEY to an admin or read-only key" >&2
  exit 2
fi

exec cargo run -q -p canary-cli -- \
  --endpoint "${CANARY_DOCTOR_ENDPOINT:-http://canary:4000}" \
  --json doctor
