# Backup Verification and Disaster Recovery

This is the production runbook for the dedicated DigitalOcean Canary host.
Canary runs as one container named `canary`, owned by `canary.service`. The
durable block volume is mounted at `/var/lib/canary` on the host and `/data` in
the container. Litestream runs inside that same container and replicates the
SQLite database to DigitalOcean Spaces through `/etc/litestream.yml`.

Generic Docker operators can use the same container-level checks with their own
SSH target, container name, and S3-compatible Litestream configuration.

## Operator Inputs

```bash
export CANARY_ENDPOINT=https://canary.mistystep.io
export CANARY_SSH_HOST=<operator-ssh-target>
export CANARY_CONTAINER=canary
# Required only for authenticated post-recovery API proof:
export CANARY_ADMIN_KEY=<securely-resolved-admin-key>
```

The SSH target is intentionally not stored in the repository. Do not infer it
from an old receipt. None of the commands below reads or prints the root-owned
container environment.

## Routine Backup Status

Verify the runtime identity and public process first:

```bash
ssh "$CANARY_SSH_HOST" sudo systemctl is-active canary.service
ssh "$CANARY_SSH_HOST" sudo docker inspect "$CANARY_CONTAINER" \
  --format '{{.Image}} {{.State.Status}} {{.State.StartedAt}}'
curl -fsS "$CANARY_ENDPOINT/healthz"
curl -fsS "$CANARY_ENDPOINT/readyz"
```

Then run the repo-owned Litestream status wrapper:

```bash
bin/dr-status
# equivalent:
bin/dr-status --host "$CANARY_SSH_HOST" --container "$CANARY_CONTAINER"
```

`bin/dr-status` sends a fixed script over SSH stdin and runs
`litestream status -config /etc/litestream.yml` inside the running container.
It is read-only. A public HTTP 200 does not replace this check.

## Non-Destructive Restore Drill

Restore the current replica into container tmpfs and require a non-empty file:

```bash
bin/dr-restore-check
# equivalent:
bin/dr-restore-check \
  --host "$CANARY_SSH_HOST" \
  --container "$CANARY_CONTAINER" \
  --db-path /data/canary.db
```

The wrapper never overwrites `/data/canary.db`; its temporary artifact is
deleted on exit. This proves restore reachability and materialization. The
production migration additionally proved a full-integrity restore and matching
application-table content before the host became authoritative.

Run both wrappers before every production image promotion and after changes to
the object-store, Litestream, volume, or host runtime.

## Configuration Contract

Production starts fail-closed with `CANARY_REQUIRE_LITESTREAM=1`. The
root-owned `/etc/canary/canary.env` supplies:

- `CANARY_DB_PATH=/data/canary.db`
- `BUCKET_NAME`
- `CANARY_REPLICA_PATH`
- `LITESTREAM_ENDPOINT`
- `LITESTREAM_REGION`
- `AWS_ACCESS_KEY_ID`
- `AWS_SECRET_ACCESS_KEY`

Never print that file. The systemd unit verifies its recorded SHA-256 before
starting the container. `/etc/canary/image.env` pins the runtime by immutable
Docker image ID, and the host preflight rejects a missing or wrong volume
mount.

## Human-Gated Recovery

Destructive recovery is never automatic. It is allowed only after
`bin/dr-status` and `bin/dr-restore-check` pass and an operator has recorded the
current image ID, volume identity, database hash/count ledger, and rollback
point.

The load-bearing order is:

1. Stop the single writer.
2. Prove no `canary` container remains and `/var/lib/canary` is the expected
   mounted volume.
3. Restore to a staged file, never over the live database.
4. Run SQLite integrity, foreign-key, schema, and application-count checks on
   the staged file.
5. Preserve the incumbent `canary.db*` files as the rollback set.
6. Atomically install the verified staged database while the writer remains
   stopped.
7. Start `canary.service`; prove public health, readiness, authenticated API
   reads, target/webhook state, and fresh Litestream status.

The first two stop gates are:

```bash
ssh "$CANARY_SSH_HOST" sudo systemctl stop canary.service
ssh "$CANARY_SSH_HOST" sudo systemctl is-active canary.service
# expected: inactive (the command itself exits non-zero)

ssh "$CANARY_SSH_HOST" sudo docker ps --filter name='^/canary$' \
  --format '{{.ID}}'
# expected: no output

ssh "$CANARY_SSH_HOST" sudo findmnt --target /var/lib/canary \
  --output SOURCE,FSTYPE,TARGET --noheadings
# expected: the dedicated ext4 volume mounted at /var/lib/canary
```

Stop if any expectation differs. Do not use `docker exec rm` against the live
container: SQLite WAL/SHM handles remain open and the operation is not a valid
restore.

The repository deliberately does not ship a one-command destructive restore.
The staged-file installation must be reviewed against the current database
schema and exact host image before execution; this prevents a stale runbook
from replacing production data merely because a storage command succeeded.

## Post-Recovery Oracle

After `sudo systemctl start canary.service`, require all of the following:

```bash
curl -fsS "$CANARY_ENDPOINT/healthz"
curl -fsS "$CANARY_ENDPOINT/readyz"
curl -fsS -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  "$CANARY_ENDPOINT/api/v1/report?window=1h" | jq '.status'
bin/dr-status
bin/dr-restore-check
bin/canary errors list canary --window 1h
```

Also compare the expected target, monitor, webhook, incident, and application
table counts from the pre-recovery ledger. HTTP health alone is not data-plane
recovery evidence.
