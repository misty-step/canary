# Networked Service Dogfooding

Canary reads a deployed-service registry from instance-local operator state:
`.canary/dogfood/owned_services.json` by default. The checked-in
[priv/dogfood/owned_services.example.json](../priv/dogfood/owned_services.example.json)
is only a starter shape. Copy it into `.canary/dogfood/owned_services.json`,
replace the example services with the operator's own apps, and keep production
service names out of committed examples.

## Audit Command

Run the checked-in audit against a live Canary instance:

```bash
bin/dogfood-audit --strict
```

The command reads `CANARY_ENDPOINT` and `CANARY_API_KEY`, validates the registry
schema, compares active HTTP services against live Canary targets, compares
active check-in services against live Canary monitors, and prints:

- the unified Canary report summary for the requested window
- every active service with target presence, URL match, health state, monitor
  state, platform, and current error totals
- pending, blocked, follow-on, suspended, and ignored services with failure mode
  and next action
- extra live targets outside the registry

Use `--window 1h` or another supported window when you want a tighter read. Use
`--json` when an agent or CI job needs a machine-readable report.

## Inventory Command

Run the deployed-surface inventory before changing dogfood coverage:

```bash
bin/dogfood-inventory --strict --json
```

The command reads the registry, any Vercel scopes passed with `--vercel-scope`,
Fly apps, and local `.vercel/project.json` links under the workspace parent. It
emits one JSON document with each surface classified as:

- `covered`: active service has health or monitor coverage and verified ingest
- `partial`: service is present but has pending, incomplete, or follow-on
  Canary coverage
- `blocked`: service is known and cannot be covered until the documented
  blocker is resolved
- `ignored`: service is explicitly out of dogfood rotation

Strict mode fails when a live Vercel/Fly deployment is missing from the
registry, an active registry service is missing from live deployment inventory,
a requested service is absent from the registry, or a collector cannot enumerate
its source. Use fixture inputs in tests or offline audits:

```bash
bin/dogfood-inventory \
  --manifest .canary/dogfood/owned_services.json \
  --vercel-projects example-team=/tmp/vercel-example-team.json \
  --fly-apps /tmp/fly-apps.json \
  --local-root /path/to/workspace \
  --requested canary-self,example-api \
  --strict --json
```

## Value Receipt Command

Run a value receipt when the question is what Canary currently proves for one
registered service:

```bash
bin/canary dogfood value --service linejam --json
```

The receipt combines registry coverage with live `/api/v1/status`
target/monitor health, error query readback, open incidents, active remediation
claims, annotations, telemetry events, and a synthetic verification verdict. It
returns exactly one `next_action` so downstream agents can continue without
re-deriving registry state.

Use the pilot pair as a regression check:

- `linejam` should render as `value_state: proven` when its target is up, query
  readback succeeds, and there are no current errors.
- `chrondle` should render as `value_state: stale_registry_evidence` when live
  readback is clean but the registry still describes the old `TypeError` flood
  triage action.

`bin/canary doctor --json` includes `response.dogfood_value` aggregate counts
for `covered`, `stale`, `blocked`, `partial`, and `value_unproven` services.

## Registry States

Each service entry has:

- `service`: Canary service name or intended service name
- `state`: `active`, `pending`, `blocked`, `follow_on`, `suspended`, or `ignored`
- `platform`: hosting/runtime platform such as `vercel`, `fly`, `azure`,
  `desktop`, or `unknown`
- `platform_project`: platform-native project/app name when it differs from
  `service`, or `null` when there is no deployment project
- `production_url`: public production surface when one exists
- `repo_path`: local checkout path when known, otherwise `null`
- `health_url`: HTTP target URL for services that expose HTTP health, or `null`
  for monitor-only services
- `monitor_mode`: `http`, `check_in`, `external`, or `none`
- `ingest_status`: `verified`, `partial`, `missing`, `not_applicable`, or
  `blocked`
- `last_checked_at`: evidence timestamp
- `failure_mode`: current blocker or "no current blocker" style status
- `owner`: accountable org or owner namespace
- `next_action`: concrete next step

`active` services must have either a non-empty `health_url` or
`monitor_mode: "check_in"`.

- `monitor_mode: "http"` with a `health_url` requires exactly one live Canary
  target for that service, a matching URL, and target state readback in
  `/api/v1/report`.
- `monitor_mode: "check_in"` requires live monitor readback and a non-empty
  `last_check_in_at`. If `health_url` is also set, strict mode verifies both
  the HTTP target and the check-in monitor.

Other states stay visible in the report but do not fail strict mode.

## Registry Lifecycle

Keep the instance-local registry current whenever an owned deployment is added,
removed, renamed, or reclassified. A service is not considered covered just
because a deployment exists or env vars exist; it needs health/monitor
enrollment and Canary readback evidence.

Historical Misty Step dogfood evidence lives under `docs/architecture/` and
backlog archives. Do not copy those service names into a clean-room deployment
unless that operator owns those apps.
