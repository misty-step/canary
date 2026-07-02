# Cold-Operator Clean-Room Receipt

Date: 2026-07-01

Goal: prove a fresh operator can follow the public deploy path without knowing
the Misty Step `canary-obs` instance, private dogfood data, or hidden
bootstrap-key recovery steps.

Scope: this is a clean-room rehearsal and repo-gate receipt, not a live
Adminifi/R90 Fly deployment transcript. It proves the checked-in defaults,
docs, and scripts no longer require Misty Step production state, and that the
production image still passes health/readiness plus SDK/write-path readback in
the repo gate. The first real Adminifi/R90 deploy should add a consumer-owned
Fly transcript without changing product defaults.

## Commands Exercised

### Instance-local dogfood registry

```bash
tmpdir=$(mktemp -d)
mkdir -p "$tmpdir/.canary/dogfood"
cp priv/dogfood/owned_services.example.json "$tmpdir/.canary/dogfood/owned_services.json"
printf '%s\n' '[{"Name":"example-canary","Organization":{"Slug":"example-org"},"Hostname":"example-canary.fly.dev"}]' > "$tmpdir/fly-apps.json"
CANARY_DOGFOOD_MANIFEST="$tmpdir/.canary/dogfood/owned_services.json" \
  bin/dogfood-inventory \
    --fly-apps "$tmpdir/fly-apps.json" \
    --local-root "$tmpdir" \
    --requested canary-self \
    --now 2026-07-01T00:00:00Z \
    --max-evidence-age-hours 1 \
    --json --strict |
  jq '{summary, requested_services, self: (.surfaces[] | select(.service == "canary-self") | {service, coverage, deployment_seen, health_url})}'
rm -rf "$tmpdir"
```

Output:

```json
{
  "summary": {
    "covered": 1,
    "partial": 2,
    "blocked": 0,
    "ignored": 0,
    "strict_failures": 0
  },
  "requested_services": [
    "canary-self"
  ],
  "self": {
    "service": "canary-self",
    "coverage": "covered",
    "deployment_seen": true,
    "health_url": "https://example-canary.fly.dev/healthz"
  }
}
```

### Local integration plan without an inherited endpoint

```bash
env -u CANARY_ENDPOINT \
  cargo run --quiet -p canary-cli -- \
  integrate plan . \
  --service canary-self \
  --production-url https://example-canary.fly.dev \
  --json |
  jq '{endpoint, service: .response.service, can_patch: .response.can_patch, target_enrollment: (.response.actions[] | select(.kind == "target_enrollment") | {status, health_url})}'
```

Output:

```json
{
  "endpoint": "https://canary.example",
  "service": "canary-self",
  "can_patch": false,
  "target_enrollment": {
    "status": "needed",
    "health_url": "https://example-canary.fly.dev"
  }
}
```

### Live CLI command without endpoint

```bash
env -u CANARY_ENDPOINT -u CANARY_API_KEY \
  cargo run --quiet -p canary-cli -- summary --json
```

Output:

```text
canary: missing Canary endpoint; set --endpoint, CANARY_ENDPOINT, or config endpoint
```

Exit status: `1`.

### Fly config validation

```bash
flyctl config validate --config fly.toml
```

Output:

```text
Validating fly.toml
✓ Configuration is valid
```

### Upstream deploy workflow app pin

```bash
sed -n '18,24p' .github/workflows/deploy.yml
```

Output:

```text
      - run: flyctl deploy --app "$FLY_APP" --remote-only
        env:
          FLY_APP: ${{ vars.CANARY_FLY_APP }}
          FLY_API_TOKEN: ${{ secrets.FLY_API_TOKEN }}
```

The checked-in placeholder in `fly.toml` does not redirect deploys because the
workflow passes an explicit app name from repository variables. On 2026-07-01,
the upstream Misty Step repository variables `CANARY_FLY_APP` and
`CANARY_WITNESS_ENDPOINT` were configured so production automation no longer
depends on checked-in `canary-obs` fallbacks.

### Repo gate and production-image smoke

```bash
./bin/validate
```

Result: exit status `0` on 2026-07-01 after rebasing onto `origin/master`.

Evidence excerpts:

```text
dogfood-audit: Results: 33 passed, 0 failed
dogfood-inventory: Results: 27 passed, 0 failed
canary-cli lib tests: 31 passed
canary-server listening on [::]:4000
/healthz: {"status":"ok"}
/readyz: {"status":"ready", ... "worker_count": 5 ...}
SDK production smoke: ingest plus query readback passed
write-path rehearsal: error_ingest, error_query_readback, report_readback,
  timeline_readback, error_detail_readback, webhook_delivery_lookup, and
  cleanup checks returned true
gitleaks dir . --redact: no leaks found
```

## Disposition

- README and `docs/self-host-fly.md` document app creation, volume creation,
  Tigris setup, deploy, first-key capture, missed-key recovery, smoke readback,
  DR checks, write-path rehearsal, and fork workflow configuration.
- `fly.toml` uses a placeholder app name. Operators must pass
  `--app "$CANARY_FLY_APP"`.
- Dogfood inventory defaults to `.canary/dogfood/owned_services.json`, with
  `priv/dogfood/owned_services.example.json` as the committed starter.
- Live CLI commands no longer fall back to the Misty Step endpoint. Local
  integration planning still has a neutral example endpoint for generated
  snippets.
- Adminifi/R90 can use this path as the clean-room deploy procedure. This
  receipt does not claim a live Adminifi/R90 deployment has already occurred.
  Backlog `020-adminifi-http-surface-verification.md` remains blocked because
  it asks a different question: whether the legacy Adminifi HTTP surfaces
  themselves are stable and canonical.
