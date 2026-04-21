# Backup, restore, and disaster recovery validation

Priority: medium
Status: done
Estimate: S

## Goal
Verify that Canary's Fly volume data can be recovered after loss or corruption,
and land a tested, CLI-first restore runbook that operators can execute without
guessing.

## Non-Goals
- Multi-region replication — single-node SQLite, single Fly region
- Automated failover — manual restore is acceptable at this scale
- Litestream setup from scratch — config already exists, this item validates it
- Mutating production Fly secrets or deploying as part of this delivery

## Constraints / Invariants
- Keep the workflow CLI-first. No dashboard-only instructions.
- Preserve the current boot contract: if the DB is missing and Litestream is
  configured, `bin/entrypoint.sh` restores before starting Canary.
- Separate non-destructive verification from destructive recovery so operators
  can prove backups exist before touching `/data/canary.db`.
- Treat missing Litestream configuration as an explicit operator-visible state,
  not an assumed happy path.

## Authority Order
tests > live `canary-obs` inspection > code > docs > backlog lore

## Repo Anchors
- `bin/entrypoint.sh` — restore-on-missing-DB and Litestream startup contract
- `test/bin/entrypoint_test.sh` — existing shell harness for entrypoint behavior
- `litestream.yml` — replica wiring and required environment variables
- `fly.toml` — Fly app name, volume mount, and process contract
- `AGENTS.md` — Fly volume/WAL footguns and current nuclear-reset sequence

## Prior Art
- `bin/bootstrap`
- `bin/validate`

## Oracle
- [x] `bash test/bin/entrypoint_test.sh` exits `0`
- [x] `bash test/bin/dr_test.sh` exits `0`
- [x] `bin/dr-status --help` exits `0`
- [x] `bin/dr-restore-check --help` exits `0`
- [x] `docs/backup-restore-dr.md` documents exact `flyctl` commands for status,
  non-destructive restore drill, and destructive recovery on Fly.io
- [x] The runbook records the initial `2026-04-16` missing-backup finding and
  the later same-day Fly Tigris activation plus verification on `canary-obs`

## Implementation Sequence
1. Add operator wrappers for remote Litestream status and a non-destructive
   restore drill.
2. Extend shell coverage for entrypoint restore behavior and helper command
   composition.
3. Add the DR runbook, cross-link it from the repo docs, and record the current
   live verification result plus the remaining operator step.

## Risk + Rollout
- Risk: operators may skip the verification drill and jump straight to
  destructive recovery. Mitigate with separate commands, warnings, and docs.
- Rollout: completed on `2026-04-16` by provisioning Fly Tigris with
  `flyctl storage create`, deploying the matching image, and rerunning the
  verification commands successfully.

## Notes
Codex flagged this during the 2026-04-01 audit. The Fly-first contract is now
`BUCKET_NAME`, `AWS_ACCESS_KEY_ID`, and `AWS_SECRET_ACCESS_KEY` via
`litestream.yml`; the older `LITESTREAM_REPLICA_URL` note is stale and the old
`LITESTREAM_*` variables remain as an entrypoint compatibility shim only.

Live inspection on `2026-04-16` found:
- `flyctl secrets list --app canary-obs` showed no `BUCKET_NAME`,
  `AWS_ACCESS_KEY_ID`, or `AWS_SECRET_ACCESS_KEY` secrets
- `flyctl ssh console --app canary-obs -C "sh -lc 'litestream status -config /etc/litestream.yml'"` failed with `bucket required for s3 replica`

Single 1 GB encrypted volume on Fly means a volume loss without working backups
is unrecoverable.

Completed on `2026-04-16` by provisioning Fly Tigris bucket
`canary-obs-backups`, deploying the matching image, and verifying both
`bin/dr-status` and `bin/dr-restore-check` against `canary-obs`.
