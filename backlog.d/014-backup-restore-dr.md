# Backup, restore, and disaster recovery validation

Priority: medium
Status: ready
Estimate: S

## Goal
Verify that Canary's data can be recovered after volume loss or corruption.
Document and test the restore procedure.

## Non-Goals
- Multi-region replication — single-node SQLite, single Fly region
- Automated failover — manual restore is acceptable at this scale
- Litestream setup from scratch — config already exists, this item validates it

## Oracle
- [ ] Given Litestream is configured, when the app is running in production, then WAL replicas are being shipped to S3 (or the configured destination)
- [ ] Given a backup exists, when the restore procedure is followed, then the database is recovered with data up to the last WAL sync
- [ ] Given the restore procedure exists, when docs are reviewed, then the exact commands for stop → restore → restart on Fly.io are documented
- [ ] Given the backup is running, when `flyctl ssh console` is used, then the operator can verify backup recency

## Notes
Codex flagged this during the 2026-04-01 audit. `LITESTREAM_REPLICA_URL` exists in
runtime.exs but there is no evidence it is active or tested in production.
Single 1GB encrypted volume on Fly — a volume loss without working backups is
unrecoverable.
