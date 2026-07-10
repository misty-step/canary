---
name: canary-deploy
description: |
  Operate Canary's dedicated-host deployment and disaster-recovery runbook:
  verify Litestream-to-S3 status, inspect the canary.service/container boundary,
  and run non-destructive restore drills. Misty Step production runs at
  canary.mistystep.io on a dedicated DigitalOcean host. Use when: "deploy
  canary", "check canary backups", "is canary backed up", "restore drill",
  "DR check", "canary disaster recovery". Trigger: /canary-deploy.
argument-hint: "[status|deploy|restore-check|nuclear-reset]"
---

# canary-deploy

Misty Step production is one Docker container named `canary`, owned by
`canary.service` on a dedicated DigitalOcean host. Caddy is the only public
ingress and serves `https://canary.mistystep.io`; the SQLite volume is mounted
on the host at `/var/lib/canary` and in the container at `/data`. Litestream
runs inside the same container and replicates to DigitalOcean Spaces through
the mounted `/etc/litestream.yml` configuration.

There is no provider auto-deploy workflow. A release is promoted only through
an explicit immutable-image update on the host after the strict gate and DR
checks pass. Full runbooks: `docs/upgrade-and-rollback.md` and
`docs/backup-restore-dr.md`.

## Surfaces

| Changed area | Surface | Verification path |
|---|---|---|
| Runtime image (`Dockerfile`, `bin/entrypoint.sh`) | `canary.service` → container `canary` | host service/container identity, then `/healthz` + `/readyz` |
| Backup/replication (`litestream.yml`, `bin/lib/dr.sh`) | Litestream → S3-compatible storage (Spaces in production) | `bin/dr-status`, `bin/dr-restore-check` |
| Public ingress | Caddy → `127.0.0.1:8080` | `https://canary.mistystep.io/healthz` and `/readyz` |

## Commands

Set the operator SSH target. It is intentionally not checked into the repo:

```sh
export CANARY_ENDPOINT=https://canary.mistystep.io
export CANARY_SSH_HOST=<operator-ssh-target>
```

Inspect the runtime without exposing environment values:

```sh
ssh "$CANARY_SSH_HOST" sudo systemctl is-active canary.service
ssh "$CANARY_SSH_HOST" sudo docker inspect canary \
  --format '{{.Image}} {{.State.Status}} {{.State.StartedAt}}'
curl -fsS "$CANARY_ENDPOINT/healthz"
curl -fsS "$CANARY_ENDPOINT/readyz"
```

Read-only backup preflight:

```sh
bin/dr-status
# equivalent: bin/dr-status --host "$CANARY_SSH_HOST" --container canary
```

Non-destructive restore drill. This restores into container tmpfs and never
overwrites the live database:

```sh
bin/dr-restore-check
# equivalent: bin/dr-restore-check --host "$CANARY_SSH_HOST" \
#   --container canary --db-path /data/canary.db
```

Before a promotion, run:

```sh
./bin/validate --strict
bin/dr-status
bin/dr-restore-check
```

Then follow the immutable-image install sequence in
`docs/upgrade-and-rollback.md`. Re-run the runtime and public probes above and
query recent Canary errors before accepting the promotion.

## Gotchas

- **Both DR scripts require operator SSH access.** They run only
  `sudo docker exec` against the named container and never read or print its
  environment.
- **`CANARY_SSH_HOST` has no default.** Missing host configuration fails loud;
  never guess an address from an old receipt.
- **The production container name is `canary`.** Override `--container` only
  for an explicitly different self-hosted install.
- **`bin/dr-restore-check` needs outbound egress** from the container to the
  configured object store. A failure means the replica is not proven usable.
- **`CANARY_REQUIRE_LITESTREAM=1`** keeps production fail-closed when the
  database is missing and restore configuration is incomplete.
- **Never delete `/data/canary.db*` in the running container.** Stop
  `canary.service`, verify the container is absent and `/var/lib/canary` is the
  durable mount, then use the reviewed recovery procedure.
- **No auto-deploy exists.** A green master build does not mutate production.

## Report

Return: **verdict** (PASS / FAIL / UNVERIFIED) · exact command(s) run ·
surface exercised (runtime / backup-status / restore-drill / public ingress) ·
artifact inspected (service state, container image ID, DR output,
`/healthz`+`/readyz`) · what was not covered.
