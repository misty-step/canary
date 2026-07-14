#!/usr/bin/env bash
set -euo pipefail

CANARY_IMAGE="${CANARY_IMAGE:-canary:canary-934}"
MINIO_IMAGE="minio/minio@sha256:a1ea29fa28355559ef137d71fc570e508a214ec84ff8083e39bc5428980b015e"
MC_IMAGE="minio/mc@sha256:aead63c77f9db9107f1696fb08ecb0faeda23729cde94b0f663edf4fe09728e3"
RUN_ID="canary-recovery-${$}-${RANDOM}"
NETWORK="$RUN_ID"
STORAGE="$RUN_ID-storage"
SOURCE="$RUN_ID-source"
RESTORED="$RUN_ID-restored"
SOURCE_VOLUME="$RUN_ID-source-data"
RESTORED_VOLUME="$RUN_ID-restored-data"
ACCESS_KEY="canary-drill-access"
SECRET_KEY="canary-drill-secret-not-production"
BUCKET="canary-recovery-drill"

cleanup() {
  docker rm -f "$SOURCE" "$RESTORED" "$STORAGE" >/dev/null 2>&1 || true
  docker volume rm -f "$SOURCE_VOLUME" "$RESTORED_VOLUME" >/dev/null 2>&1 || true
  docker network rm "$NETWORK" >/dev/null 2>&1 || true
}
trap cleanup EXIT

fail() {
  printf 'portable recovery drill: %s\n' "$1" >&2
  exit 1
}

wait_for_storage() {
  local attempt
  for attempt in $(seq 1 30); do
    if docker run --rm --network "$NETWORK" "$MC_IMAGE" \
      alias set drill http://storage:9000 "$ACCESS_KEY" "$SECRET_KEY" \
      >/dev/null 2>&1; then
      return
    fi
    sleep 1
  done
  fail "object storage did not become ready"
}

wait_for_canary() {
  local container="$1" attempt
  for attempt in $(seq 1 60); do
    if docker exec "$container" curl -fsS http://127.0.0.1:4000/readyz \
      >/dev/null 2>&1; then
      return
    fi
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || true)" != "true" ]]; then
      docker logs "$container" >&2 || true
      fail "$container stopped before becoming ready"
    fi
    sleep 1
  done
  docker logs "$container" >&2 || true
  fail "$container did not become ready"
}

list_backup_objects() {
  docker run --rm --network "$NETWORK" \
    -e "MC_HOST_drill=http://$ACCESS_KEY:$SECRET_KEY@storage:9000" \
    "$MC_IMAGE" ls --recursive "drill/$BUCKET"
}

runtime_args=(
  --network "$NETWORK"
  -e CANARY_REQUIRE_LITESTREAM=1
  -e CANARY_DISCLOSE_BOOTSTRAP_KEY=false
  -e BUCKET_NAME="$BUCKET"
  -e CANARY_REPLICA_PATH=canary.db
  -e LITESTREAM_ENDPOINT=http://storage:9000
  -e LITESTREAM_REGION=us-east-1
  -e AWS_ACCESS_KEY_ID="$ACCESS_KEY"
  -e AWS_SECRET_ACCESS_KEY="$SECRET_KEY"
)

docker network create "$NETWORK" >/dev/null
docker volume create "$SOURCE_VOLUME" >/dev/null
docker volume create "$RESTORED_VOLUME" >/dev/null
docker run -d --network "$NETWORK" --network-alias storage --name "$STORAGE" \
  -e MINIO_ROOT_USER="$ACCESS_KEY" \
  -e MINIO_ROOT_PASSWORD="$SECRET_KEY" \
  "$MINIO_IMAGE" server /data >/dev/null
wait_for_storage
docker run --rm --network "$NETWORK" \
  -e "MC_HOST_drill=http://$ACCESS_KEY:$SECRET_KEY@storage:9000" \
  "$MC_IMAGE" mb "drill/$BUCKET" >/dev/null

initial_verification="$(docker run --rm -v "$SOURCE_VOLUME:/data" \
  --entrypoint /app/bin/canary-server "$CANARY_IMAGE" \
  migrate --database /data/canary.db)"
admin_key="$(docker run --rm -v "$SOURCE_VOLUME:/data" \
  --entrypoint /app/bin/canary-server "$CANARY_IMAGE" \
  mint-key --scope admin --name recovery-drill 2>/dev/null)"
[[ -n "$admin_key" ]] || fail "failed to mint disposable API key"

docker run -d --name "$SOURCE" "${runtime_args[@]}" \
  -v "$SOURCE_VOLUME:/data" "$CANARY_IMAGE" >/dev/null
wait_for_canary "$SOURCE"

ingest_response="$(docker exec -e CANARY_DRILL_KEY="$admin_key" "$SOURCE" sh -c '
  curl -fsS -X POST http://127.0.0.1:4000/api/v1/errors \
    -H "Authorization: Bearer $CANARY_DRILL_KEY" \
    -H "Content-Type: application/json" \
    -d "{\"service\":\"recovery-drill\",\"error_class\":\"ExpectedDrillSignal\",\"message\":\"portable recovery proof\",\"severity\":\"low\"}"
')"
error_id="$(jq -r '.id // empty' <<<"$ingest_response")"
[[ -n "$error_id" ]] || fail "ingest did not return an error id"

object_listing=""
for _ in $(seq 1 30); do
  object_listing="$(list_backup_objects)"
  [[ -n "$object_listing" ]] && break
  sleep 1
done
[[ -n "$object_listing" ]] || fail "replication produced no backup objects"

# A clean stop gives Litestream time to flush the final WAL segment.
docker stop --time 30 "$SOURCE" >/dev/null
object_listing="$(list_backup_objects)"
object_count="$(wc -l <<<"$object_listing" | tr -d '[:space:]')"

docker rm "$SOURCE" >/dev/null
docker volume rm "$SOURCE_VOLUME" >/dev/null

docker run -d --name "$RESTORED" "${runtime_args[@]}" \
  -v "$RESTORED_VOLUME:/data" "$CANARY_IMAGE" >/dev/null
wait_for_canary "$RESTORED"

health="$(docker exec "$RESTORED" curl -fsS http://127.0.0.1:4000/healthz)"
readiness="$(docker exec "$RESTORED" curl -fsS http://127.0.0.1:4000/readyz)"
runtime="$(docker exec "$RESTORED" /app/bin/canary-server version)"
verification="$(docker exec "$RESTORED" /app/bin/canary-server \
  verify-data --database /data/canary.db)"
restored_error="$(docker exec -e CANARY_DRILL_KEY="$admin_key" "$RESTORED" sh -c \
  'curl -fsS -H "Authorization: Bearer $CANARY_DRILL_KEY" "http://127.0.0.1:4000/api/v1/errors/'"$error_id"'"')"

jq -n \
  --arg image_id "$(docker image inspect "$CANARY_IMAGE" -f '{{.Id}}')" \
  --argjson initial_verification "$initial_verification" \
  --argjson health "$health" \
  --argjson readiness "$readiness" \
  --argjson runtime "$runtime" \
  --argjson verification "$verification" \
  --argjson restored_error "$restored_error" \
  --argjson backup_object_count "$object_count" \
  '{
    schema: "canary.portable-recovery-drill.v1",
    status: "ok",
    image_id: $image_id,
    source: {migration_and_data: $initial_verification},
    backup: {object_count: $backup_object_count},
    restored: {
      health: $health,
      readiness: $readiness,
      runtime: $runtime,
      data_verification: $verification,
      error: {id: $restored_error.id, service: $restored_error.service,
              error_class: $restored_error.error_class}
    }
  }
  | if .restored.error.service != "recovery-drill"
       or .restored.error.error_class != "ExpectedDrillSignal"
       or .restored.data_verification.schema_current != true
       or .restored.data_verification.integrity_check != "ok"
       or .backup.object_count < 1
    then error("portable recovery acceptance failed") else . end'
