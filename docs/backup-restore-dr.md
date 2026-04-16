# Backup, Restore, and DR

This runbook covers Litestream backup verification and manual recovery for the
Fly app `canary-obs`.

## Current State

Initial live inspection on `2026-04-16` found that `canary-obs` was not
actively backing up:

- `flyctl secrets list --app canary-obs` showed no `BUCKET_NAME`,
  `AWS_ACCESS_KEY_ID`, or `AWS_SECRET_ACCESS_KEY` secrets
- the same remote Litestream preflight later wrapped by `bin/dr-status` failed
  with `bucket required for s3 replica`

Later on `2026-04-16`, Fly Tigris was provisioned with
`flyctl storage create --app canary-obs --name canary-obs-backups --yes`, the
matching image was deployed, and live verification succeeded:

- `bin/dr-status` reported `/data/canary.db` as `ok`
- `bin/dr-restore-check` materialized a temporary `10M` restore file on the
  running machine

Treat the commands below as the ongoing source of truth before any destructive
recovery work.

The shell tests in this repo verify wrapper command composition and
restore-on-missing-DB behavior. They do not replace a live Fly/Litestream
verification run against `canary-obs`.

## Enable Fly Tigris Backups

Provision a Fly-managed Tigris bucket for `canary-obs`:

```bash
flyctl storage create --app canary-obs --name canary-obs-backups --yes
```

`flyctl storage create` provisions the bucket and stages or deploys the app
secrets for Fly's Tigris object storage. Canary only depends on
`BUCKET_NAME`, `AWS_ACCESS_KEY_ID`, and `AWS_SECRET_ACCESS_KEY`; the Tigris
endpoint and `auto` region are intentionally pinned in `litestream.yml` so the
backup contract stays Fly-specific and small.

Confirm the secrets are present:

```bash
flyctl secrets list --app canary-obs
```

If Fly reports them as `Staged`, deploy the current release with the new
secrets without rebuilding:

```bash
flyctl secrets deploy --app canary-obs
```

`bin/dr-status` is still the authoritative verification step because it checks
the running container's effective Litestream configuration.

## Read-Only Backup Verification

Check Litestream replication status on the running machine:

```bash
bin/dr-status
```

The wrapper owns the exact `flyctl ssh console` contract through
[`bin/lib/dr.sh`](../bin/lib/dr.sh), so quoting changes stay single-source in
code instead of being hand-copied into the runbook.

When backup configuration is missing, this command fails non-zero. That is the
expected signal to fix Fly Tigris configuration before doing any DR work.

## Non-Destructive Restore Drill

Restore the Tigris replica to a temporary file on the running machine without
touching the live database:

```bash
bin/dr-restore-check
```

The wrapper builds the remote restore command through
[`bin/lib/dr.sh`](../bin/lib/dr.sh) so the shell quoting contract only lives in
one place.

Treat this as the required proof that a replica can be materialized before any
destructive recovery on `/data/canary.db`. `bin/dr-restore-check --db-path ...`
passes the path as a positional argument to the remote shell so Fly does not
misparse it as a command prefix. The drill now fails unless Litestream leaves a
non-empty restored database artifact.

## Destructive Recovery

Use this only after `bin/dr-status` and `bin/dr-restore-check` succeed.

1. Capture the current Fly machine ID, volume ID, image, and region from Fly's JSON output.
2. Stop the real app machine so SQLite releases its WAL file handles and the
   volume can be mounted elsewhere safely.
3. Start a temporary maintenance machine with the same image, no traffic, and
   the stopped `/data` volume attached.
4. Delete the local DB files from that maintenance machine and verify they are
   actually gone.
5. Destroy the maintenance machine so the volume is detached again.
6. Start the real app machine so `bin/entrypoint.sh` restores from Litestream
   before Canary starts.

```bash
set -euo pipefail

MACHINE_JSON=$(flyctl machines list --app canary-obs --json | jq -ce 'map(select(.config.env.FLY_PROCESS_GROUP == "app")) | .[0]')
MACHINE_ID=$(printf '%s' "$MACHINE_JSON" | jq -er '.id')
VOLUME_ID=$(printf '%s' "$MACHINE_JSON" | jq -er '.config.mounts[0].volume')
IMAGE=$(printf '%s' "$MACHINE_JSON" | jq -er '.config.image')
REGION=$(printf '%s' "$MACHINE_JSON" | jq -er '.region')
MAINT_NAME="cdrm_$(date +%s | tail -c 7)"

printf 'Recovering Fly machine: %s (volume %s)\n' "$MACHINE_ID" "$VOLUME_ID"

flyctl machines stop "$MACHINE_ID" --app canary-obs

flyctl machine run --app canary-obs --region "$REGION" --detach \
  --skip-dns-registration --restart no --name "$MAINT_NAME" \
  --entrypoint sleep --volume "$VOLUME_ID":/data "$IMAGE" infinity

MAINT_ID=
for _ in $(seq 1 30); do
  MAINT_ID=$(flyctl machines list --app canary-obs --json | jq -er --arg name "$MAINT_NAME" '.[] | select(.name == $name) | .id' 2>/dev/null || true)
  [ -n "$MAINT_ID" ] && break
  sleep 2
done
test -n "$MAINT_ID"

cleanup_maint() {
  flyctl machine destroy "$MAINT_ID" --app canary-obs --force
}
trap cleanup_maint EXIT

flyctl machine wait "$MAINT_ID" --app canary-obs --state started
flyctl machine exec "$MAINT_ID" "sh -eu -c 'rm -f /data/canary.db /data/canary.db-wal /data/canary.db-shm && for path in /data/canary.db /data/canary.db-wal /data/canary.db-shm; do [ ! -e \"$path\" ] || { echo \"$path still present\" >&2; exit 1; }; done'" --app canary-obs

flyctl machine destroy "$MAINT_ID" --app canary-obs --force
trap - EXIT

flyctl machines start "$MACHINE_ID" --app canary-obs
```

Why the stop/maintenance/start sequence exists:

- stopping the machine releases SQLite's WAL file handles
- the temporary maintenance machine keeps the volume writable without running
  Canary against that volume
- the final start is what re-runs `bin/entrypoint.sh` with the database paths
  missing, which triggers the Litestream restore path

This maintenance-machine flow was validated on `2026-04-16` against a
disposable forked volume using the same image and `flyctl machine run` /
`flyctl machine exec` sequence above.

## Post-Recovery Checks

Re-run the backup checks after the machine comes back:

```bash
bin/dr-status
bin/dr-restore-check
```

Check recent logs for the restore message:

```bash
flyctl logs --app canary-obs --no-tail | rg "Restoring database from Litestream"
```

Treat this as a failed recovery and stop immediately if it appears:

```bash
flyctl logs --app canary-obs --no-tail | rg "did not materialize /data/canary.db"
```

Then verify the service itself:

```bash
curl -fsS https://canary-obs.fly.dev/healthz
curl -fsS https://canary-obs.fly.dev/readyz
```
