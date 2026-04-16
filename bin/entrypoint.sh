#!/bin/bash
set -e

DB_PATH="${CANARY_DB_PATH:-/data/canary.db}"

# --- Litestream env validation ---
LITESTREAM_READY=0
if [ -z "${BUCKET_NAME:-}" ]; then
  echo "WARNING: Litestream replication NOT configured — BUCKET_NAME missing, running without backups" >&2
else
  MISSING=""
  [ -z "${AWS_ACCESS_KEY_ID:-}" ] && MISSING="$MISSING AWS_ACCESS_KEY_ID"
  [ -z "${AWS_SECRET_ACCESS_KEY:-}" ] && MISSING="$MISSING AWS_SECRET_ACCESS_KEY"

  if [ -n "$MISSING" ]; then
    echo "WARNING: Fly Tigris bucket set but missing required variables:$MISSING" >&2
  else
    LITESTREAM_READY=1
  fi
fi

# Restore from Litestream if DB doesn't exist locally
if [ ! -f "$DB_PATH" ] && [ "$LITESTREAM_READY" = "1" ]; then
  echo "Restoring database from Litestream..."
  litestream restore -if-replica-exists -o "$DB_PATH" -config /etc/litestream.yml "$DB_PATH"

  if [ ! -s "$DB_PATH" ]; then
    echo "ERROR: Litestream restore did not materialize $DB_PATH — refusing to start on an empty database" >&2
    exit 1
  fi
fi

# Start app under Litestream (continuous replication)
if [ "$LITESTREAM_READY" = "1" ]; then
  exec litestream replicate -exec "/app/bin/canary start" -config /etc/litestream.yml
else
  exec /app/bin/canary start
fi
