# Upgrade and Rollback

Canary is a single-process, single-database service. Production upgrades are an
explicit immutable-image replacement on the dedicated DigitalOcean host;
rollback is a previous immutable image plus, only when a schema changed, a
pre-upgrade database restore. There is no automatic provider deploy workflow.

## Production Contract

- endpoint: `https://canary.mistystep.io`
- host supervisor: `canary.service`
- container: `canary`
- durable host mount: `/var/lib/canary`
- container database: `/data/canary.db`
- image pin: `/etc/canary/image.env` (`CANARY_IMAGE_ID=sha256:...`)
- backups: Litestream in the container to DigitalOcean Spaces

Set the operator inputs without checking the SSH target into the repo:

```bash
export CANARY_ENDPOINT=https://canary.mistystep.io
export CANARY_SSH_HOST=<operator-ssh-target>
```

## Pre-Upgrade Gate

```bash
./bin/validate --strict
bin/dr-status
bin/dr-restore-check
ssh "$CANARY_SSH_HOST" sudo systemctl is-active canary.service
ssh "$CANARY_SSH_HOST" sudo docker inspect canary \
  --format '{{.Image}} {{.State.Status}} {{.State.StartedAt}}'
```

Record the incumbent image ID and a pre-upgrade database/replica receipt before
changing the host. If either DR wrapper fails, stop.

## Build and Stage an Immutable Image

Build for the host architecture from the exact reviewed commit. The release
version is passed explicitly; local/gate builds otherwise report
`0.0.0-dev`.

```bash
commit=$(git rev-parse HEAD)
version=$(git describe --tags --always --dirty)
archive="canary-${commit}-linux-amd64.docker.tar"

docker buildx build \
  --platform linux/amd64 \
  --provenance=false \
  --build-arg "CANARY_VERSION=$version" \
  --tag "canary:$commit" \
  --output "type=docker,dest=$archive" \
  .
sha256sum "$archive"
```

Transfer the archive over the authenticated SSH channel. On the host, verify
the recorded archive hash, load it, inspect its `amd64` architecture, and write
only the resulting immutable image ID to `/etc/canary/image.env`. This is the
same pin consumed by the existing `canary-image-preflight`; never put a mutable
tag in that file.

The image install is intentionally an operator-reviewed transaction because it
changes production. Do not put the archive hash or image tag into a shell
command until they have been copied from the build receipt.

## Activate and Verify

Restart the existing systemd unit after the immutable image pin is installed:

```bash
ssh "$CANARY_SSH_HOST" sudo systemctl restart canary.service
ssh "$CANARY_SSH_HOST" sudo systemctl is-active canary.service
ssh "$CANARY_SSH_HOST" sudo docker inspect canary \
  --format '{{.Image}} {{.State.Status}} {{.State.StartedAt}}'

curl -fsS "$CANARY_ENDPOINT/healthz"
curl -fsS "$CANARY_ENDPOINT/readyz"
curl -fsS -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  "$CANARY_ENDPOINT/api/v1/report?window=1h" | jq '.status'
bin/canary errors list canary --window 1h
bin/dr-status
```

The container image ID must equal the reviewed new pin. Public health without
that identity match is not deployment evidence.

## Schema Migrations

Schema migrations are forward-only. `Store::migrate` runs on boot and stamps
`user_version` only after applying missing migrations. It fails closed on
partial existing schemas.

There is no automated schema rollback. If a migration introduces a problem,
stop `canary.service`, restore the verified pre-upgrade database under the
human-gated procedure in `docs/backup-restore-dr.md`, restore the previous
immutable image ID in `/etc/canary/image.env`, and start the service.

## Image-Only Rollback

When the schema is unchanged:

1. Stop `canary.service`.
2. Restore the previously recorded immutable image ID to
   `/etc/canary/image.env`.
3. Start `canary.service`.
4. Re-run the full activation oracle above.

Do not select a rollback image by a mutable tag. The host preflight accepts
only an immutable `sha256:` image ID that is already loaded.

## Pre-Upgrade Checklist

- [ ] `./bin/validate --strict` green on the target commit
- [ ] `bin/dr-status` passes
- [ ] `bin/dr-restore-check` passes
- [ ] no open `service=canary` incident
- [ ] incumbent image ID and database/replica receipt recorded
- [ ] new linux/amd64 archive hash and resulting immutable image ID recorded
- [ ] schema compatibility reviewed

## What Not to Do

- Do not delete the database file while `canary.service` is running.
- Do not run two containers against `/var/lib/canary`.
- Do not weaken `CANARY_REQUIRE_LITESTREAM=1` to make an upgrade start.
- Do not treat a green GitHub build as a production deployment.
