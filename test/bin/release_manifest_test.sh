#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/bin/release-manifest"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

MANIFEST="$TMP/release.json"
$SCRIPT generate \
  --version v1.2.3 \
  --commit 0123456789abcdef0123456789abcdef01234567 \
  --oci-repository registry.example/canary \
  --oci-digest sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  --created-at 2026-07-14T12:00:00Z \
  --output "$MANIFEST"
$SCRIPT verify --file "$MANIFEST"
jq -e '
  .artifact.reference == "registry.example/canary@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  and .compatibility.automatic_previous_image_rollback == false
' "$MANIFEST" >/dev/null

jq '.artifact.digest = "latest"' "$MANIFEST" > "$TMP/invalid.json"
if "$SCRIPT" verify --file "$TMP/invalid.json" >/dev/null 2>&1; then
  echo "invalid mutable artifact reference was accepted" >&2
  exit 1
fi

for path in source artifact contracts compatibility; do
  jq --arg path "$path" '.[$path].unexpected = true' "$MANIFEST" > "$TMP/nested-extra.json"
  if "$SCRIPT" verify --file "$TMP/nested-extra.json" >/dev/null 2>&1; then
    echo "unexpected nested key was accepted in $path" >&2
    exit 1
  fi
done

jq '.created_at = "2026-02-30T12:00:00Z"' "$MANIFEST" > "$TMP/impossible-date.json"
if "$SCRIPT" verify --file "$TMP/impossible-date.json" >/dev/null 2>&1; then
  echo "semantically impossible created_at was accepted" >&2
  exit 1
fi

if grep -Eqi 'digitalocean|tigris|(^|[^[:alnum:]_])fly([^[:alnum:]_]|$)|misty step|ssh|systemd|(^|[^[:alnum:]_])(host|mount|port)([^[:alnum:]_]|$)' \
  "$ROOT/contracts/release-manifest.v1.schema.json" "$MANIFEST"; then
  echo "release manifest contract contains deployment topology" >&2
  exit 1
fi

echo "portable release manifest tests passed"
