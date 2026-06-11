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

## Registry States

Each service entry has:

- `service`: Canary service name or intended service name
- `state`: `active`, `pending`, `blocked`, `follow_on`, `suspended`, or `ignored`
- `platform`: hosting/runtime platform such as `vercel`, `fly`, `azure`,
  `desktop`, or `unknown`
- `production_url`: public production surface when one exists
- `health_url`: HTTP target URL for active services, or `null` for non-HTTP or
  not-yet-healthable services
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
| `volume` | unknown | `https://www.volume.fitness/api/health` | Live audit reported the target down; investigate or reclassify. |
| `vulcan` | Fly | `https://adminifi-vulcan-orchestrator.fly.dev/health` | Active Adminifi orchestrator surface. |

Pending or blocked services include `sploot`, `misty-step`, `vanity`,
`trump-goggles-splash`, `timeismoney-splash`, `adminifi-web`, and
`consumer-portal`. Follow-on services include desktop/non-HTTP or unpinned
surfaces such as `time-tracker` and `cerberus`.

Keep the registry current whenever an owned deployment is added, removed,
renamed, or reclassified. A service is not considered covered just because a
deployment exists or env vars exist; it needs health/monitor enrollment and
Canary readback evidence.
