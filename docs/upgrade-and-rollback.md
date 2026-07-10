# Upgrade and Rollback

Canary is a single-process, single-database service. Upgrades are
image-and-deploy; rollback is image-and-restore. There are no
zero-downtime rolling deploys, blue/green pools, or schema rollback
scripts — the design is intentionally simpler than a clustered system.

## Upgrade Path

1. **Verify backups before touching anything:**

   ```bash
   bin/dr-status --app "$CANARY_FLY_APP"
   bin/dr-restore-check --app "$CANARY_FLY_APP"
   ```

   If either fails, do not upgrade. Fix backup configuration first.

2. **Pull the latest master and build:**

   ```bash
   git pull origin master
   flyctl deploy --app "$CANARY_FLY_APP" --remote-only
   ```

3. **Verify after deploy:**

   ```bash
   curl -fsS "$CANARY_ENDPOINT/healthz"
   curl -fsS "$CANARY_ENDPOINT/readyz"
   curl -fsS -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
     "$CANARY_ENDPOINT/api/v1/report?window=1h" | jq '.status'
   ```

   Also check for `service=canary` errors in the post-deploy window:

   ```bash
   bin/canary errors list canary --window 1h
   ```

## Schema Migrations

Schema migrations are **forward-only**. `Store::migrate` runs on boot
and stamps `user_version` after applying missing migrations. It fails
closed on partial existing schemas (see `CLAUDE.md` footgun: "Schema
ownership").

There is no automated schema rollback. If a migration introduces a
problem:

1. Stop the machine (`flyctl machines stop`).
2. Restore the database from the pre-upgrade Litestream replica (see
   `docs/backup-restore-dr.md` → Destructive Recovery).
3. Deploy the previous image.

## Rollback

### Image Rollback (no data change)

If the new code is broken but the schema is unchanged:

```bash
flyctl deploy --app "$CANARY_FLY_APP" --remote-only --image <previous-image-digest>
```

Or use Fly's built-in rollback:

```bash
flyctl image rollback --app "$CANARY_FLY_APP"
```

### Data Rollback (schema changed)

If the new version applied a schema migration and you need to revert:

1. Stop the machine.
2. Follow the Destructive Recovery procedure in
   `docs/backup-restore-dr.md`.
3. Deploy the previous image. `Store::migrate` on the old image will
   see the restored `user_version` and skip forward migrations.

**Important:** The restored database must come from a Litestream replica
taken *before* the upgrade. If the only available replica is from after
the migration ran, the old image may fail to read the newer schema.

## Pre-Upgrade Checklist

- [ ] `bin/dr-status` passes
- [ ] `bin/dr-restore-check` passes (non-destructive drill)
- [ ] No open `service=canary` incidents
- [ ] `./bin/validate --strict` green on the target commit
- [ ] Operator has the previous image digest recorded for rollback

## What Not to Do

- Do not delete the database file while the app is running. SQLite WAL
  keeps file handles open; see `CLAUDE.md` footgun: "SQLite WAL and
  `rm -f`".
- Do not run two versions against the same database simultaneously.
  The single-writer invariant assumes one process owns the store.
- Do not skip the backup verification step. "Restore-based DR" means
  the backup is the rollback — if it is stale or missing, there is no
  safety net.
