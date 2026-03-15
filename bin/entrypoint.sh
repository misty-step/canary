#!/bin/bash
set -e

DB_PATH="${CANARY_DB_PATH:-/data/canary.db}"

# --- Litestream env validation ---
if [ -z "$LITESTREAM_S3_BUCKET" ]; then
  echo "WARNING: Litestream replication NOT configured — running without backups" >&2
else
  MISSING=""
  [ -z "$LITESTREAM_ACCESS_KEY_ID" ] && MISSING="$MISSING LITESTREAM_ACCESS_KEY_ID"
  [ -z "$LITESTREAM_SECRET_ACCESS_KEY" ] && MISSING="$MISSING LITESTREAM_SECRET_ACCESS_KEY"
  [ -z "$LITESTREAM_S3_REGION" ] && MISSING="$MISSING LITESTREAM_S3_REGION"

  if [ -n "$MISSING" ]; then
    echo "WARNING: Litestream S3 bucket set but missing required variables:$MISSING" >&2
  fi
fi

# Restore from Litestream if DB doesn't exist locally
if [ ! -f "$DB_PATH" ] && [ -n "$LITESTREAM_S3_BUCKET" ]; then
  echo "Restoring database from Litestream..."
  litestream restore -if-replica-exists -o "$DB_PATH" -config /etc/litestream.yml "$DB_PATH"
fi

# Start app under Litestream (continuous replication)
if [ -n "$LITESTREAM_S3_BUCKET" ]; then
  exec litestream replicate -exec "/app/bin/canary start" -config /etc/litestream.yml
else
  exec /app/bin/canary start
fi
