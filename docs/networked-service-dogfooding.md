# Networked Service Dogfooding

Canary keeps a checked-in deployed-service registry at
[priv/dogfood/owned_services.json](../priv/dogfood/owned_services.json).
The registry is the operator contract for which owned services Canary should
monitor, which services are pending or blocked, and what action moves each one
forward.

## Audit Command

Run the checked-in audit against a live Canary instance:

```bash
bin/dogfood-audit --strict
```

The command reads `CANARY_ENDPOINT` and `CANARY_API_KEY`, validates the registry
schema, compares active HTTP services against live Canary targets, and prints:

- the unified Canary report summary for the requested window
- every active service with target presence, URL match, health state, platform,
  and current error totals
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

The command reads the registry, Vercel projects for scopes `misty-step` and
`adminifi-growth`, Fly apps, and local `.vercel/project.json` links under the
workspace parent. It emits one JSON document with each surface classified as:

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
  --manifest priv/dogfood/owned_services.json \
  --vercel-projects misty-step=/tmp/vercel-misty-step.json \
  --fly-apps /tmp/fly-apps.json \
  --local-root /Users/phaedrus/Development \
  --requested canary-self,vanity,chrondle,linejam,misty-step,trump-goggles-splash,timeismoney-splash,sploot \
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
- `health_url`: HTTP target URL for active services, or `null` for non-HTTP or
  not-yet-healthable services
- `monitor_mode`: `http`, `check_in`, `external`, or `none`
- `ingest_status`: `verified`, `partial`, `missing`, `not_applicable`, or
  `blocked`
- `last_checked_at`: evidence timestamp
- `failure_mode`: current blocker or "no current blocker" style status
- `owner`: accountable org or owner namespace
- `next_action`: concrete next step

`active` services must have a non-empty `health_url`; strict audit fails if the
live Canary target is missing, duplicated, or pointed at another URL. Other
states stay visible in the report but do not fail strict mode.

## Current Registry

As of 2026-06-11, active registry services are:

| Service | Platform | Health URL | Notes |
|---|---|---|---|
| `canary-self` | Fly | `https://canary-obs.fly.dev/healthz` | Self HTTP liveness is enrolled; independent witness is tracked separately. |
| `chrondle` | Vercel | `https://www.chrondle.app/api/health` | Enrolled, but the live 24h audit showed a high-volume `TypeError` group. |
| `linejam` | Vercel | `https://www.linejam.app/api/health` | Reference integration with Vercel health and Fly responder coverage. |
| `vulcan` | Fly | `https://adminifi-vulcan-orchestrator.fly.dev/health` | Active Adminifi orchestrator surface. |

Pending or blocked services include `sploot`, `misty-step`, `vanity`,
`trump-goggles-splash`, `timeismoney-splash`, `adminifi-web`, and
`consumer-portal`. Follow-on services include desktop/non-HTTP or unpinned
surfaces such as `time-tracker` and `cerberus`. Ignored services are explicitly
out of rotation; `volume` is ignored because the product is retired and its
public Vercel surface now returns `DEPLOYMENT_NOT_FOUND`.

Keep the registry current whenever an owned deployment is added, removed,
renamed, or reclassified. A service is not considered covered just because a
deployment exists or env vars exist; it needs health/monitor enrollment and
Canary readback evidence.
