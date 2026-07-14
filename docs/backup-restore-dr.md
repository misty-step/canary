# Backup, Restore, and Data Verification

Canary's recovery contract is portable. It defines Litestream replication to
S3-compatible storage, a non-destructive restore drill, transactional migration
of a disposable copy, and deterministic SQLite verification. It does not choose
a storage provider or encode any deployment topology.

The canonical commands, inputs, evidence schema, and recovery invariants are in
[`portable-runtime-contract.md`](portable-runtime-contract.md#backup-and-restore).

The product deliberately does not ship a destructive installation command.
After `bin/canary-recovery restore-check` succeeds, the deployer must separately
authorize stopping the single writer, preserving the incumbent database,
installing the verified restored file atomically, starting the selected image,
and proving health, version, and data readback. Those actions depend on the
deployer's runtime and recovery policy and therefore stay outside Canary.
