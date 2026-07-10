# Self-Host Canary With Docker

This is the provider-independent path for a fresh Canary instance. It uses the
same checked-in Docker image as production, a local Docker volume for SQLite,
and no cloud-provider account or provider-specific environment variables.

## Prerequisites

- Docker with Compose v2
- `curl` and `jq` on the host for the copy-paste smoke commands
- This repository checkout

## Start Canary

Build and start the service:

```bash
docker compose up --build -d canary
```

The service listens on `http://localhost:4000` and stores its SQLite database in
the named Docker volume `canary_canary-data`.

If you prefer raw Docker without Compose:

```bash
docker build -t canary:local .
docker volume create canary-data
docker run -d --name canary \
  -p 4000:4000 \
  -v canary-data:/data \
  -e CANARY_DB_PATH=/data/canary.db \
  -e CANARY_REQUIRE_LITESTREAM=0 \
  canary:local
```

## Capture The Bootstrap Admin Key

On first boot, Canary prints a one-time bootstrap admin key to container logs.
Capture it immediately and store it in your secret manager; it is not shown
again after the database has been seeded.

```bash
docker compose logs canary | grep "Bootstrap API key:"
export CANARY_ADMIN_KEY="<store-the-bootstrap-key-securely>"
```

If you missed the first boot log, mint a replacement admin key against the
existing container database. This prints a raw key once:

```bash
docker compose exec canary \
  /app/bin/canary-server mint-key --scope admin --name operator-recovery
```

## First-Boot Smoke

Set the local endpoint and prove the process and writable store are live:

```bash
export CANARY_ENDPOINT="http://localhost:4000"
curl -fsS "$CANARY_ENDPOINT/healthz"
curl -fsS "$CANARY_ENDPOINT/readyz"
```

Run the agent-facing doctor immediately after first boot. From a workstation
with the Rust toolchain available:

```bash
CANARY_ENDPOINT="$CANARY_ENDPOINT" \
CANARY_API_KEY="$CANARY_ADMIN_KEY" \
bin/canary doctor --json
```

For a Docker-only machine, run the Compose doctor profile instead. It uses a
Rust container and named Cargo cache volumes, so no Rust installation is needed
on the host:

```bash
export CANARY_API_KEY="$CANARY_ADMIN_KEY"
docker compose run --rm canary-doctor
```

Do not pass the key with `-e CANARY_API_KEY=<value>` on the `docker compose
run` command line — that puts the raw key in the host's process list for
every process running `docker compose run` while it executes. `docker-compose.yml`
declares `CANARY_API_KEY` with no inline value, so Compose passes it through
from your shell's own exported environment instead.

Treat any non-clean doctor output as a failed production setup until the
reported field is understood and either fixed or explicitly waived. A local
smoke with no external witness may report `canary-watchman monitor is missing`;
that proves the container is reachable, but the instance is not production-ready
until self-watch is configured.

## Ingest And Query Roundtrip

Create scoped caller keys instead of sharing the bootstrap admin key:

```bash
CANARY_INGEST_KEY="$(
  curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/keys" \
    -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
    -H "Content-Type: application/json" \
    -d '{"name":"docker-smoke-ingest","scope":"ingest-only"}' \
    | jq -r '.key'
)"

CANARY_READ_KEY="$(
  curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/keys" \
    -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
    -H "Content-Type: application/json" \
    -d '{"name":"docker-smoke-read","scope":"read-only"}' \
    | jq -r '.key'
)"
```

Ingest one synthetic error and read it back:

```bash
curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/errors" \
  -H "Authorization: Bearer $CANARY_INGEST_KEY" \
  -H "Content-Type: application/json" \
  -d '{"service":"docker-smoke","error_class":"DockerSmoke","message":"docker self-host smoke"}'

curl -fsS "$CANARY_ENDPOINT/api/v1/query?service=docker-smoke&window=1h" \
  -H "Authorization: Bearer $CANARY_READ_KEY" \
  | jq '.groups | length'
```

The final command should print a number greater than zero.

## Optional Backups

The default Docker path runs without Litestream backups. That is intentional for
the smallest local self-host path: `CANARY_REQUIRE_LITESTREAM=0` and no
`BUCKET_NAME` means the entrypoint starts Canary directly with a local volume.

The entrypoint treats these backup variables as the portable S3-compatible
contract:

- `BUCKET_NAME`
- `AWS_ACCESS_KEY_ID`
- `AWS_SECRET_ACCESS_KEY`
- `CANARY_REQUIRE_LITESTREAM=1` to fail closed when backup configuration is
  incomplete

The checked-in `litestream.yml` reads its endpoint and region from
`LITESTREAM_ENDPOINT` and `LITESTREAM_REGION`. For DigitalOcean Spaces, MinIO,
plain AWS S3, or another S3-compatible endpoint, set those variables and mount
the desired Litestream config at `/etc/litestream.yml`.

Plain AWS S3 example config:

```yaml
dbs:
  - path: /data/canary.db
    replicas:
      - type: s3
        bucket: ${BUCKET_NAME}
        path: canary.db
        region: us-east-1
```

MinIO example config:

```yaml
dbs:
  - path: /data/canary.db
    replicas:
      - type: s3
        bucket: ${BUCKET_NAME}
        path: canary.db
        endpoint: http://minio:9000
        region: us-east-1
        force-path-style: true
```

When backups are required, start only after proving restore status from the
mounted configuration:

```bash
docker compose exec canary litestream status -config /etc/litestream.yml
```

## Stop Or Reset The Local Instance

Stop the service while keeping data:

```bash
docker compose down
```

Delete the local database volume and force a fresh bootstrap:

```bash
docker compose down -v
```

Do not delete `/data/canary.db*` while the container is running. SQLite WAL file
handles stay open; stop the container before destructive maintenance.
