# Rust production cutover evidence: 2026-06-06

This packet records the first Fly production cutover from the Phoenix release
image to the Rust `canary-server` image. It is an evidence receipt, not a
completion claim for the full Rust rewrite.

## Commit under test

- Branch: `rewrite/rust-canary`
- Commit: `5292835 feat(rust): cut production image to server`
- Production image: `registry.fly.io/canary-obs:deployment-01KTEY1W968E1RCDHNBD9K042J`
- Fly app: `canary-obs`
- Machine: `78407d7f515008`
- Fly machine version after deploy: `69`

## Preflight

Commands:

```bash
git status --short --branch --untracked-files=all
bin/dr-status
flyctl secrets list --app canary-obs
```

Evidence:

- Worktree was clean before deploy.
- `bin/dr-status` reported `/data/canary.db` as `ok` before deploy.
- Required Litestream/Tigris secret names were deployed: `BUCKET_NAME`,
  `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_ENDPOINT_URL_S3`, and
  `AWS_REGION`.

## Deploy

Command:

```bash
NO_COLOR=1 flyctl deploy --app canary-obs --remote-only --detach
```

Evidence:

- Fly validated `fly.toml`.
- Depot built the Rust Docker image from `Dockerfile`.
- `cargo build --release --locked -p canary-server` completed inside the image
  build.
- The final image size was `46 MB`.
- Fly updated machine `78407d7f515008` with a rolling strategy and reported the
  machine in a good state.

## Runtime smoke

Commands:

```bash
flyctl status --app canary-obs
curl -fsS https://canary-obs.fly.dev/healthz
curl -fsS https://canary-obs.fly.dev/readyz
bin/dr-status
flyctl ssh console --app canary-obs -C 'sh -lc "ls -l /app/bin /data"'
flyctl logs --app canary-obs --no-tail
```

Evidence:

- `flyctl status` reported image
  `canary-obs:deployment-01KTEY1W968E1RCDHNBD9K042J`.
- `flyctl status` reported `2 total, 2 passing` checks for machine version 69.
- `/healthz` returned `{"status":"ok"}`.
- `/readyz` returned
  `{"status":"ready","checks":{"database":"ok","supervisor":"ok"}}`.
- `bin/dr-status` reported `/data/canary.db` as `ok` after deploy.
- Fly SSH showed `/app/bin/canary-server` and `/app/bin/entrypoint.sh` in the
  running image, plus `/data/canary.db`, `/data/canary.db-shm`, and
  `/data/canary.db-wal` on the mounted volume.
- Logs showed `/app/bin/entrypoint.sh` as the process command, Litestream
  replication to the Fly Tigris endpoint, `canary-server listening on
  0.0.0.0:4000`, and both Fly HTTP checks changing to passing after startup.

## Production Canary and Sploot QA

Commands used an existing local `CANARY_API_KEY` only as an Authorization
header; no key values were written to this packet.

```bash
curl -fsS -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/status?service=sploot-web&window=24h"

curl -fsS -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/query?service=sploot-web&window=24h"

curl -fsS -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/query?service=sploot-web&window=7d"

curl -fsS -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/timeline?service=sploot-web&window=7d&limit=20"
```

Evidence:

- Authenticated read routes responded successfully after the Rust cutover.
- `canary-self` remained `up`; the overall status stayed `unhealthy` because
  the unrelated `volume` target was already down with HTTP 404 health checks.
- Direct `service=sploot` queries returned no events or errors; the live service
  key is `sploot-web`.
- `sploot-web` had `0` errors in the last `1h`.
- `sploot-web` had `16` errors across `2` classes in the last `24h`; most
  frequent was `UnknownError` with `14` occurrences, last seen at
  `2026-06-06T15:28:12.152127Z`, sample message `[object Object]`.
- `sploot-web` had `64` errors across `8` classes in the last `7d`; most
  frequent was `PrismaClientKnownRequestError` with `21` occurrences.
- `sploot-web` had `79` errors across `10` classes in the last `30d`.
- The `7d` timeline returned `19` Sploot events, including incident updates,
  new `Error`, new `PrismaClientInitializationError`, new
  `PrismaClientKnownRequestError`, and an `UnknownError` regression.

## Residual risks

- This proves the Rust server is serving production behind Fly for the checked
  public and read-only routes. It does not prove every admin, ingest, webhook,
  retention, target-probe, TLS-scan, and monitor path under production load.
- The repository still contains the Phoenix/Elixir service and tests as parity
  sources, fixture generators, SDK boundary, and migration safety net. Removing
  them requires a separate Rust-owned migration/fixture/DR plan.
- `flyctl logs --no-tail` still includes old Phoenix shutdown log lines before
  the Rust process starts. The post-start evidence is the Rust listener line,
  the running image contents, and the passing health/readiness checks.
- The production status route currently reports an unrelated `volume` target
  down. That is not a Rust cutover regression, but it keeps Canary's aggregate
  status unhealthy.
