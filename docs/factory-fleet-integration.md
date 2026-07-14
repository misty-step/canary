# Factory Fleet Integration

This is the 15-minute path for a Factory repo to report health, uptime, and
an error or check-in signal to a Canary deployment. Use it from the consumer
repo; keep service-specific code changes in that repo.

> **Rust apps reporting their own errors/check-ins from inside the process**
> (services, workers, CLIs, build tools) should follow
> [`rust-consumer-integration.md`](rust-consumer-integration.md) — the
> in-process reporter pattern (env-gated `src/canary.rs`, error ingest +
> check-in loop, fleet-proven against `memory-engine-canary` and
> `bitterblossom/src/canary.rs`). This file is the operator-side enrollment
> path for URL-polled HTTP targets.

## Inputs

- `CANARY_ENDPOINT`: the deployment's externally supplied Canary URL, for
  example `https://canary.example`.
- A read/admin operator key for inspection and enrollment. Do not print it.
- The service name agents should query, for example `powder`.
- One production health URL when the app exposes HTTP health.
- One check-in monitor name when the app has a worker, scheduler, CLI, or
  process heartbeat.

Prefer canonical HTTPS health URLs. Private target probes require an explicit
route from the Canary runtime plus `ALLOW_PRIVATE_TARGETS=true` in its reviewed
runtime environment. Routing, placement, and restart policy belong to the
deployment owner, not this repository or a consumer repo.

## Path

1. Inspect current coverage from the consumer repo.

```bash
/path/to/canary/bin/canary integrate status /path/to/app \
  --service <service> \
  --production-url <health-url> \
  --json
```

2. Verify the health URL from the deployed Canary runtime when the URL is
private. The infrastructure owner supplies the execution mechanism.

```bash
curl -fsS https://<private-health-url>/healthz
```

Public URLs can be verified with a normal `curl -fsS <url>`.

3. Enroll the HTTP target.

```bash
/path/to/canary/bin/canary integrate enroll \
  --service <service> \
  --url <health-url> \
  --project-root /path/to/app \
  --json
```

`--project-root` writes `.canary/integration.json` in the consumer repo. Use it
only from that repo's own lane so resident work is not overwritten.

4. Enroll a check-in monitor for non-HTTP uptime, then send one proof check-in.

```bash
curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/monitors" \
  -H "Authorization: Bearer $CANARY_ADMIN_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"name":"<service>-worker","service":"<service>","mode":"ttl","expected_every_ms":300000,"grace_ms":120000}'

curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/check-ins" \
  -H "Authorization: Bearer $CANARY_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"monitor":"<service>-worker","status":"alive","summary":"fleet integration proof"}'
```

The app-owned steady-state version should use a scoped ingest key, not an admin
key. The operator may use an admin key for a one-time proof because admin keys
also satisfy ingest authority.

5. Read back live proof.

```bash
curl -fsS -H "Authorization: Bearer $CANARY_READ_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/report?window=24h" \
  | jq '.targets,.monitors'

/path/to/canary/bin/dogfood-audit \
  --manifest /path/to/instance/owned_services.json \
  --strict --json
```

The service is integrated only when readback shows either:

- HTTP target coverage: target present, URL matches, and report includes a
  target state.
- Check-in coverage: monitor present, report includes a monitor state, and
  `last_check_in_at` is non-empty.

For Factory composition, use both when the app has both an HTTP health surface
and a non-HTTP runtime heartbeat.

## Status Receipt

Each Factory repo ends in one of these states:

- `integrated`: live Canary readback proves the target and/or monitor signal.
- `intentionally deferred`: the repo is not an active runtime app for this
  fleet slice, or the responsible resident lane owns the app-side patch.
- `blocked`: the missing production URL, credential, route, or resident-lane
  boundary is named without printing secret values.

The receipt should include the service name, health URL or monitor name,
target/monitor IDs when created, the exact readback command, and the next app
lane action.
