---
name: canary-deploy
description: |
  Operate Canary's deploy and disaster-recovery runbook: verify Litestream
  backup status before touching anything, deploy to the Fly app
  (`canary-obs`), and run non-destructive restore drills. Canary is a
  self-hosted Rust service on Fly with a single-writer SQLite DB replicated
  to Tigris via Litestream. Use when: "deploy canary", "check canary
  backups", "is canary backed up", "restore drill", "DR check", "canary
  disaster recovery". Trigger: /canary-deploy.
argument-hint: "[status|deploy|restore-check|nuclear-reset]"
---

<!--
Generated via harness-kit's repo-local skill generation pattern
(skills/harness-engineering/references/repo-local-skill-generation.md).
Source repo: misty-step/canary @ c8e281b. Generated: 2026-07-01.
Generator ref: harness-kit@cbe82137.
Facts below are repo-derived at generation time, not invented. Re-verify
commands against the live repo before trusting this if it has aged — a
generated skill is a snapshot, not a live view.
-->

# canary-deploy

Deploy is auto-triggered on green `master` (`.github/workflows/deploy.yml`
runs `flyctl deploy` after CI's `workflow_run` succeeds); the manual path
exists for out-of-band redeploys. The load-bearing discipline here is DR
verification, not the deploy command itself — `bin/dr-status` and
`bin/dr-restore-check` are read-only/non-destructive checks that must both
pass before any destructive recovery is even considered. Full narrative
runbook: `docs/backup-restore-dr.md`.

## Surfaces

| Changed area | Surface | Verification path |
|---|---|---|
| Deploy config (`fly.toml`, `Dockerfile`, `.github/workflows/deploy.yml`) | Fly app `canary-obs` | `flyctl deploy --app canary-obs --remote-only`, then `/healthz` + `/readyz` |
| Backup/replication (`litestream.yml`, `bin/entrypoint.sh`, `bin/lib/dr.sh`) | Litestream → Fly Tigris | `bin/dr-status`, `bin/dr-restore-check` |

## Commands

Set the target app once (both DR scripts read `CANARY_FLY_APP`/`FLY_APP`;
neither has a hardcoded default — see `bin/lib/dr.sh:dr_default_app`):

```sh
export CANARY_FLY_APP=canary-obs
```

Read-only backup preflight (run this before anything else — exits non-zero
if Litestream config is missing):

```sh
bin/dr-status
# equivalent: bin/dr-status --app canary-obs
```

Non-destructive restore drill (restores the Tigris replica to a temp file on
the running machine; never touches the live DB; needs outbound egress from
the Fly machine):

```sh
bin/dr-restore-check
# equivalent: bin/dr-restore-check --app canary-obs --db-path /data/canary.db
```

Manual deploy (normally automatic on green master via the `Deploy` workflow):

```sh
flyctl deploy --app canary-obs --remote-only
```

Provision Tigris backups on a fresh app (one-time; `flyctl storage create`
stages/deploys the `BUCKET_NAME`/`AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`
secrets Canary reads):

```sh
flyctl storage create --app canary-obs --name canary-obs-backups --yes
flyctl secrets list --app canary-obs   # confirm secrets present, not "Staged"
```

## Gotchas

- **`bin/dr-status`/`bin/dr-restore-check` need `flyctl ssh console` access**
  to the real Fly app — they are live production preflights, not local
  fixtures. Both are safe to run against `canary-obs` at any time (read-only
  / temp-file-only), but they are not sandboxed.
- **Neither DR script has a hardcoded app default.** Forgetting
  `CANARY_FLY_APP`/`--app` fails loud with "Missing Fly app" rather than
  silently targeting the wrong instance — do not work around that by
  guessing an app name.
- **`bin/dr-restore-check` needs outbound egress** from the Fly machine to
  reach Tigris; failure here means the replica cannot be trusted for
  recovery, not just that the drill script is broken.
- **`CANARY_REQUIRE_LITESTREAM=1`** makes `bin/entrypoint.sh` fail-closed on
  boot if the DB is missing and Litestream can't restore it — relevant when
  diagnosing a boot failure after a volume issue.
- **Never skip straight to Destructive Recovery** (`docs/backup-restore-dr.md`,
  stop-machine → mount elsewhere → delete `/data/canary.db*` → restart)
  without both `bin/dr-status` and `bin/dr-restore-check` passing first — that
  sequence is explicitly human-gated and must not be automated.
- **`bin/dr.sh`'s quoting contract is single-sourced** — do not hand-write
  the equivalent `flyctl ssh console` command; use the wrapper so quoting
  changes stay in one place.

## Report

Return: **verdict** (PASS / FAIL / UNVERIFIED) · exact command(s) run ·
surface exercised (deploy / backup-status / restore-drill) · artifact
inspected (dr-status output, restore-check output, `/healthz`+`/readyz`
response) · what was NOT covered (e.g. "backup status only — no restore
drill run").
