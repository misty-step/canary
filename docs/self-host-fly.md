# Self-Host Canary On Fly

This is the cold-operator path for a fresh Canary instance. It does not depend
on Misty Step's production app, private secrets, or checked-in dogfood data.

## Prerequisites

- `flyctl` authenticated to the operator's Fly organization
- Docker-compatible builder access for `flyctl deploy --remote-only`
- `jq`, `curl`, Rust, Node.js, and Dagger for local validation
- A unique Fly app name owned by the operator

Use explicit env vars throughout the run:

```bash
export CANARY_FLY_APP="<your-fly-app>"
export CANARY_ENDPOINT="https://${CANARY_FLY_APP}.fly.dev"
mkdir -p .canary/dogfood
cp priv/dogfood/owned_services.example.json .canary/dogfood/owned_services.json
```

Do not run deploy, DR, or witness commands without setting
`CANARY_FLY_APP`/`FLY_APP` or passing `--app`.

## Create The Fly App

```bash
flyctl apps create "$CANARY_FLY_APP"
flyctl volumes create canary_data \
  --app "$CANARY_FLY_APP" \
  --region iad \
  --size 1
```

The checked-in `fly.toml` describes the process, mount, and health checks, but
its app name is a placeholder. Always pass `--app "$CANARY_FLY_APP"` so a fork
or local checkout does not target another operator's instance.

## Configure Tigris Backups

Create the Fly Tigris bucket and verify Litestream secrets exist:

```bash
flyctl storage create \
  --app "$CANARY_FLY_APP" \
  --name "${CANARY_FLY_APP}-backups" \
  --yes

flyctl secrets list --app "$CANARY_FLY_APP" | grep -E 'BUCKET_NAME|AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY'
flyctl secrets set --app "$CANARY_FLY_APP" CANARY_REQUIRE_LITESTREAM=1
```

`bin/entrypoint.sh` refuses startup when `CANARY_REQUIRE_LITESTREAM=1` and the
backup configuration is incomplete.

## Deploy And Capture The First Admin Key

```bash
flyctl deploy --app "$CANARY_FLY_APP" --remote-only
flyctl logs --app "$CANARY_FLY_APP" --no-tail | grep -E 'Bootstrap API key:'
```

The first boot seed logs the bootstrap admin key once. Store it in the
operator's secret manager and do not paste it into receipts or issues:

```bash
export CANARY_ADMIN_KEY="<store-the-bootstrap-key-securely>"
```

If the first boot log was missed, mint a replacement admin key directly against
the existing SQLite store. This does not delete or reset data:

```bash
CANARY_ADMIN_KEY="$(
  flyctl ssh console --app "$CANARY_FLY_APP" \
    -C '/app/bin/canary-server mint-key --scope admin --name operator-recovery' \
    2>/dev/null | tail -n 1
)"
```

Store the recovered key securely. The command prints the raw key once.

## Initialize Dogfood Registry

Canary's dogfood registry is instance-local operator state, not product source.
Start with the example, then replace every service with the operator's own
apps before running strict audits:

```bash
mkdir -p .canary/dogfood
cp priv/dogfood/owned_services.example.json .canary/dogfood/owned_services.json
$EDITOR .canary/dogfood/owned_services.json
```

The default audit path reads `.canary/dogfood/owned_services.json`. Use
`CANARY_DOGFOOD_MANIFEST` or `--manifest` for a different file:

```bash
bin/dogfood-inventory \
  --vercel-scope "$VERCEL_TEAM_SLUG" \
  --requested canary-self,first-service \
  --strict --json

bin/dogfood-audit \
  --endpoint "$CANARY_ENDPOINT" \
  --api-key "$CANARY_ADMIN_KEY" \
  --strict --json
```

## Smoke The Instance

```bash
curl -fsS "$CANARY_ENDPOINT/healthz"
curl -fsS "$CANARY_ENDPOINT/readyz"
curl -fsS "$CANARY_ENDPOINT/api/v1/report?window=1h" \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY"
```

Create scoped keys for callers instead of sharing the admin key:

```bash
curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/keys" \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "first-service-ingest", "scope": "ingest-only"}'

curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/keys" \
  -H "Authorization: Bearer $CANARY_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "operator-read", "scope": "read-only"}'
```

## Verify DR And Write Paths

```bash
NO_COLOR=1 bin/dr-status --app "$CANARY_FLY_APP"
NO_COLOR=1 bin/dr-restore-check --app "$CANARY_FLY_APP"

bin/canary-write-path-rehearsal \
  --endpoint "$CANARY_ENDPOINT" \
  --api-key "$CANARY_ADMIN_KEY" \
  --app "$CANARY_FLY_APP" \
  --json
```

For a local or non-Fly rehearsal, pass `--no-dr-status` instead of letting the
script guess an app.

## Configure Fork Workflows

Forks are safe to leave unconfigured. The deploy workflow runs only when
`vars.CANARY_FLY_APP` is set. The witness workflow runs only when
`vars.CANARY_WITNESS_ENDPOINT` is set. The upstream Misty Step production app
is configured through those repository variables, not checked-in defaults.

For a fork-owned production instance, configure:

- Repository variables: `CANARY_FLY_APP`, `CANARY_WITNESS_ENDPOINT`
- Repository secrets: `FLY_API_TOKEN`, `CANARY_WITNESS_READ_KEY`,
  `CANARY_WITNESS_INGEST_KEY`

Then run the witness once manually and confirm it uploads
`canary-witness-receipt.json` for the operator's own endpoint.
