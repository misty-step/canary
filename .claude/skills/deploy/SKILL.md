---
name: deploy
description: |
  Ship merged code to a deploy target. Thin router — detects target from
  repo config, dispatches to platform-specific recipe, captures a
  structured receipt (sha, version, URL, rollback handle), stops when the
  target reports healthy. Does not monitor (→ /monitor), does not triage
  (→ /diagnose), does not decide when to deploy.
  Use when: "deploy", "ship this", "ship to prod", "release", "push to
  staging", "deploy this branch", "release cut".
  Trigger: /deploy, /ship-it, /release.
argument-hint: "[--env <name>] [--version <ref>] [--rollback] [--dry-run]"
---

# /deploy

Ship merged `master` to Fly app `canary-obs` (region `iad` primary). One
invocation, one target, one receipt. The skill is a thin router around
`flyctl deploy --app canary-obs --remote-only`; it captures a receipt, waits
for `GET /healthz` and `GET /readyz` on `https://canary-obs.fly.dev` to go
green, and stops. It does not monitor (→ `/monitor`), triage (→ `/diagnose`),
or decide when to ship.

## Execution Stance

You are the executive orchestrator for a narrow, high-stakes action against
a single-writer SQLite database replicated to Fly Tigris.

- Keep the abort/ship decision on the lead model. Do not delegate go/no-go
  on a deploy that touches `canary_data` volume and live Litestream replication.
- Delegate preflight checks (`flyctl auth whoami`, `bin/dr-status`, CI
  lookup, receipt field construction) to subagents. Run them in parallel.
- The `flyctl deploy` call itself is serial and blocking.
- The nuclear reset sequence in `docs/backup-restore-dr.md` is
  **human-gated**. Never automate it.

## Contract

**Input:** merged ref to ship (default: current `HEAD` on `master`).
Optional `--version <ref>` for hotfix rollforward. Optional `--rollback`.
There is exactly one environment (`prod`) and one target (Fly app
`canary-obs`), so `--env` is accepted but effectively ignored.

**Output:** a deploy receipt (schema below) emitted to stdout as JSON and
written to `.evidence/deploys/<date>/<sha-short>.json`. Appended to
`.spellbook/cycle-manifest.json` as `deploy_receipts[]` when `/flywheel` is
the caller.

**Stops at:** both `GET https://canary-obs.fly.dev/healthz` and
`GET https://canary-obs.fly.dev/readyz` return 2xx AND `flyctl status --app
canary-obs` reports the machine as `started` on the new image sha within
`rollback_grace_seconds` (default 300).

**Does NOT:**
- Monitor post-deploy (→ `/monitor`)
- Triage failures or spiking error ingest (→ `/diagnose`)
- Auto-rollback — emits the `flyctl releases rollback` command, does not run it
- Rotate the bootstrap API key (one-shot on first boot only)
- Touch Litestream credentials — those are managed via
  `flyctl secrets set` + `flyctl storage create`, not from this skill
- Execute the nuclear reset in `docs/backup-restore-dr.md`

## Protocol

### 1. Detect target

Canary has exactly one deploy target. Detection is a sanity check, not a
multi-way router.

Required files at repo root:
1. `fly.toml` present with `app = "canary-obs"` and `primary_region = "iad"`
2. `Dockerfile` present (two-stage build: Elixir 1.17 build → debian bookworm runtime with Litestream copied from `litestream/litestream:latest`)
3. `litestream.yml` present, endpoint pinned to `https://fly.storage.tigris.dev` (not generic S3)
4. `bin/entrypoint.sh` present — restores from Litestream on empty `/data/canary.db` then `exec`s `litestream replicate -exec "/app/bin/canary start"`

If any of those are missing or `fly.toml` names a different app, abort.
This skill does not deploy anywhere except `canary-obs`.

### 2. Validate (parallel)

Dispatch in parallel. All must pass before `flyctl deploy` fires:

- **Ref exists & merged:** `git rev-parse --verify <version>` resolves AND
  the commit is an ancestor of `origin/master`.
- **CI green:** `gh run list --workflow=ci.yml --branch=master --limit=1 --json headSha,conclusion`
  reports `success` for `<version>` sha. The full gate is Dagger `strict`
  via the hosted `pull_request_target` control plane (see
  `docs/ci-control-plane.md`); locally reproduce with
  `./bin/validate --strict`.
- **Target reachable:** `flyctl auth whoami` returns an identity with access to `canary-obs`.
- **No secrets in diff:** quick scan of `git show <sha>` for token
  patterns. Bootstrap API keys (`"Bootstrap API key:"` log lines)
  never appear in source — if one does, abort and rotate.
- **Storage preflight:** `flyctl secrets list --app canary-obs` contains
  `BUCKET_NAME`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`. Missing
  means Tigris was never provisioned — run
  `flyctl storage create --app canary-obs --name canary-obs-backups --yes`
  first (see `docs/backup-restore-dr.md` § "Enable Fly Tigris Backups").
  If Fly reports secrets as `Staged`, run `flyctl secrets deploy --app canary-obs`.
- **DR preflight:** `bin/dr-status` exits zero (read-only Litestream
  replication status on the running machine) AND `bin/dr-restore-check`
  exits zero (non-destructive restore drill that materializes the Tigris
  replica to a temp file without touching `/data/canary.db`). These are
  the Canary-specific equivalent of "can I roll forward safely" — if
  replication is broken, no deploy.
- **Current state:** `flyctl status --app canary-obs --json` → capture
  current image ref, machine ID, and region. This feeds step 3 and 4.

Hosted deploy path: `.github/workflows/deploy.yml` runs on
`workflow_run` success of `ci.yml` on `master` and executes
`flyctl deploy --remote-only` with `FLY_API_TOKEN`. When that pipeline is
healthy, the local invocation of this skill should be idempotent against
the sha the workflow already shipped (→ step 3).

### 3. Idempotence check

If `flyctl status --app canary-obs --json` reports a currently-running
image whose embedded commit sha == `<version>` sha: skip the deploy. Emit
a receipt with `action: "no-op"` and the current release as
`rollback_handle`. This is the success path when `/flywheel` re-invokes
`/deploy` on a sha that `.github/workflows/deploy.yml` already shipped.

### 4. Capture rollback handle BEFORE deploy

Run `flyctl releases list --app canary-obs --json | jq '.[0].version'` and
store the result as `rollback_handle`. This is the version ID that
`flyctl releases rollback <id> --app canary-obs` will consume. If Fly
does not surface a prior release (first-ever deploy), record
`rollback_handle: null` and set `first_deploy: true` in the receipt — the
operator has been warned that this ship is irreversible via CLI.

### 5. Dispatch

Run, serially, blocking on completion:

```bash
flyctl deploy --app canary-obs --remote-only
```

`--remote-only` forces the build on Fly's builders (no local Docker
required). The Dockerfile is a two-stage Elixir 1.17 → debian bookworm
build with Litestream embedded. Release goes through the `canary_data`
volume mounted at `/data`.

Stream the deploy logs through Fly, but cap captured output in the
receipt at the last 80 lines. Full log firehose goes to stderr for the
operator's terminal; receipts stay small.

### 6. Wait for healthy

Poll with exponential backoff up to `rollback_grace_seconds` (default
300) until **all three** return green:

```bash
curl -fsS https://canary-obs.fly.dev/healthz
curl -fsS https://canary-obs.fly.dev/readyz
flyctl status --app canary-obs --json | jq -e '.Machines[] | select(.state == "started")'
```

`/healthz` is the Fly HTTP service check configured in `fly.toml`
(`grace_period = "15s"`, `interval = "30s"`). `/readyz` is Canary's own
readiness signal — it's what downstream responders (e.g. bitterblossom)
watch before trusting timeline queries and webhook deliveries. Both must
be 2xx.

If the grace window expires:
- Emit receipt with `status: "unhealthy"` and the concrete rollback
  command in `rollback_command`:
  `flyctl releases rollback <rollback_handle> --app canary-obs`
- Do **not** auto-rollback. Exit non-zero.
- The operator or `/monitor` decides whether to reverse.

### 7. First-boot bootstrap key ritual

Applies only when `first_deploy: true` in step 4. On the very first boot
of `canary-obs`, Canary logs a one-shot bootstrap API key to stdout:

```bash
flyctl logs --app canary-obs --no-tail | rg "Bootstrap API key:"
```

**This key cannot be re-shown.** Capture it immediately into the
operator's secret manager. It's the only credential that can create
further scoped keys (`ingest-only` / `read-only` / `admin`) through the
API. On subsequent deploys this step is skipped — the key lives in the
SQLite DB (replicated via Litestream) and is not reprinted.

Do not log the actual key value into the deploy receipt. Record only
`bootstrap_key_captured: true|false` so the presence of the ritual is
auditable without leaking the secret.

### 8. Emit receipt

Write JSON to stdout. Append to `.spellbook/cycle-manifest.json` if it
exists. Also write to `.evidence/deploys/<date>/<sha-short>.json`.

## Receipt Schema

```json
{
  "version": "abc1234",
  "sha": "abc1234567890...",
  "env": "prod",
  "target": "fly",
  "app": "canary-obs",
  "region": "iad",
  "url": "https://canary-obs.fly.dev",
  "healthcheck_url": "https://canary-obs.fly.dev/healthz",
  "readyz_url": "https://canary-obs.fly.dev/readyz",
  "machine_id": "d8d393a1b12345",
  "deploy_id": "01HX...",
  "rollback_handle": "v42",
  "rollback_command": "flyctl releases rollback v42 --app canary-obs",
  "status": "healthy",
  "action": "deployed",
  "first_deploy": false,
  "bootstrap_key_captured": false,
  "dr_status_ok": true,
  "dr_restore_check_ok": true,
  "timestamp": "2026-04-20T14:32:10Z",
  "duration_seconds": 94,
  "operator": "phrazzld"
}
```

Field rules:
- `status` ∈ {`healthy`, `unhealthy`, `timeout`, `aborted`}
- `action` ∈ {`deployed`, `no-op`, `rolled-back`, `aborted`}
- `rollback_handle` MUST be non-null except when `first_deploy: true`
  (no prior Fly release) or `action == "aborted"` pre-step-4
- `dr_status_ok` and `dr_restore_check_ok` MUST be `true` when
  `action == "deployed"`. A green Canary deploy without a green
  replication preflight is a recipe for unrecoverable data loss
  given the single-writer SQLite architecture
- `sha` is the full 40-char sha; `version` is the short form

## Rollback Mode

`/deploy --rollback [--to <release-id>]` — reverse the most recent
Fly release.

- Default `<release-id>`: `rollback_handle` from the most recent receipt
  in `.evidence/deploys/`.
- Dispatch: `flyctl releases rollback <release-id> --app canary-obs`.
- Confirm `/healthz` + `/readyz` green post-rollback against the same
  grace window as step 6.
- Emit a new receipt with `action: "rolled-back"`.
- Do NOT chain rollbacks. Reversing further requires an explicit
  `--to <earlier-release-id>` from `flyctl releases list --app canary-obs`.

Rollback only reverses the Fly release (image + config). It does **not**
reverse schema migrations or data already persisted to `/data/canary.db`.
Migrations on `Canary.Repo` (pool_size:1, single-writer) are additive by
convention — a release rollback against a newer schema is generally safe
because old code treats new columns as absent. If a deploy added a
destructive migration, rollback alone will not restore prior state; the
destructive recovery runbook in `docs/backup-restore-dr.md` applies.

## Nuclear Reset (human-gated, do NOT automate)

When the DB is corrupt beyond rolling forward and the Tigris replica is
the canonical source of truth, the tested sequence lives in
`docs/backup-restore-dr.md` § "Destructive Recovery". The skill must
**never** execute it. Reason: SQLite WAL keeps `/data/canary.db` open on
the live machine, so `rm -f` on the running app is a no-op. The safe
sequence:

1. `flyctl machines stop <machine-id> --app canary-obs` — releases WAL handles
2. `flyctl machine run --app canary-obs --region iad --detach --skip-dns-registration --restart no --name cdrm_<suffix> --entrypoint sleep --volume <volume-id>:/data <image> infinity` — maintenance machine, no traffic
3. `flyctl machine exec <maint-id> "sh -eu -c 'rm -f /data/canary.db /data/canary.db-wal /data/canary.db-shm'" --app canary-obs`
4. `flyctl machine destroy <maint-id> --app canary-obs --force` — detach the volume cleanly
5. `flyctl machines start <machine-id> --app canary-obs` — re-runs `bin/entrypoint.sh`, which triggers the Litestream restore path

Validated `2026-04-16` against a disposable forked volume. If the skill
detects conditions that suggest nuclear reset is needed (sustained
`/readyz` failure after green `/healthz`, or evidence of DB corruption
in logs), it emits a receipt with `status: "unhealthy"`, names
`docs/backup-restore-dr.md`, and exits. A human runs the sequence.

## Gotchas

- **Fly endpoint `port:` footgun.** `config/runtime.exs` must include
  explicit `port: String.to_integer(System.get_env("PORT") || "4000")`
  in the `http:` keyword list of the prod `CanaryWeb.Endpoint` config.
  A second `config :canary, CanaryWeb.Endpoint` block **replaces** the
  `http:` key rather than merging — omitting `port:` causes Phoenix to
  bind to a random port and Fly's healthcheck on `:4000` fails silently.
  If `/healthz` times out at step 6 on a green build, this is the
  first thing to check.
- **SQLite WAL + `rm -f` is a no-op on a live machine.** Every footgun
  in Canary eventually traces back to this. If you find yourself about
  to delete the DB on a running machine, stop and read
  `docs/backup-restore-dr.md`.
- **Litestream credential drift.** `BUCKET_NAME`, `AWS_ACCESS_KEY_ID`,
  `AWS_SECRET_ACCESS_KEY` are the only secrets Canary depends on for
  replication. The Tigris endpoint and `auto` region are pinned in
  `litestream.yml` on purpose — do not parameterize them. If a secret
  rotation leaves them `Staged`, `flyctl secrets deploy --app canary-obs`
  republishes without rebuilding.
- **Tigris pinning.** `litestream.yml` uses
  `endpoint: https://fly.storage.tigris.dev` with `force-path-style: true`
  and `region: auto`. Canary does not support generic S3 in production;
  Fly Tigris is the only blessed backend.
- **`bin/entrypoint.sh` hard-fails empty restores.** If Litestream
  restore runs but materializes an empty file, the entrypoint exits 1
  rather than starting on a blank database. Read `flyctl logs` for
  `did not materialize /data/canary.db` — that's a poisoned replica,
  not a cold start.
- **`Health.Manager` boot race.** `lib/canary/health/manager.ex` uses
  `rescue` in `handle_info(:boot)` to retry in 5s if the targets table
  isn't ready. On a first-ever deploy, `/readyz` may flap for ~5s
  after `/healthz` goes green. Don't mistake this for a failure —
  wait one retry cycle before escalating.
- **Oban Lite tables.** If `/readyz` fails with a missing `oban_jobs`
  table, the Ecto migration `priv/repo/migrations/20260314230000_create_oban_jobs.exs`
  didn't run. Oban's SQLite engine does not auto-create tables; a
  dedicated migration does. This is a pre-deploy bug, not a deploy bug.
- **Idempotence preserves Fly billing budget.** `flyctl deploy` on an
  unchanged sha still triggers a builder pass. Step 3 must short-circuit
  before step 5.
- **Hosted CI pushes deploys.** `.github/workflows/deploy.yml` fires on
  `workflow_run` after `ci.yml` green on `master`. Local `/deploy` will
  frequently hit the no-op path because CI already shipped. That's
  working-as-designed, not a bug.
- **`flyctl ssh console` vs. destructive work.** `bin/dr-status` and
  `bin/dr-restore-check` wrap `flyctl ssh console` through
  `bin/lib/dr.sh` so the quoting contract lives in one place. Do not
  inline the SSH commands here — drift between the skill and the
  wrappers will silently break preflight.
- **Bootstrap key is one-shot.** If you miss it on first deploy, you
  can re-bootstrap only via the nuclear reset path. Capture the
  `"Bootstrap API key:"` line the moment step 7 sees it.

## Related

- `/flywheel` — outer-loop caller; passes merged sha on `master`
- `/monitor` — consumes this receipt; watches `/healthz` + `/readyz` +
  error ingest rate post-deploy, decides on rollback
- `/diagnose` — triages anomalies, reads `lib/canary/errors/ingest.ex`
  and incident timelines
- `/settle` / `/land` — merge gate. `./bin/validate --strict` must be
  green before `/deploy` can find a green CI run to honor
- `./bin/validate` — the project gate (flags: `--fast`, `--strict`,
  `--advisories`). `--strict` is what `deploy.yml` implicitly trusts
- `docs/backup-restore-dr.md` — canonical DR runbook; nuclear reset
  sequence; Tigris provisioning
- `docs/ci-control-plane.md` — hosted `pull_request_target` immutable
  control plane that gates `deploy.yml`
- `bin/dr-status`, `bin/dr-restore-check` — Litestream preflight
  scripts invoked in step 2
- `.github/workflows/deploy.yml` — automated deploy path; this skill
  is the manual + idempotent counterpart
