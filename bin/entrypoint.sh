#!/bin/bash
set -e

DB_PATH="${CANARY_DB_PATH:-/data/canary.db}"

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
