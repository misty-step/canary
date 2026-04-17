# Networked Service Dogfooding

This repo already had live dogfood traffic before backlog item `007` was picked
up. The missing piece was a trustworthy operator view: which owned HTTP
services are actually under Canary right now, which ones are only documented in
other repos, and how to verify the difference from this repo without guessing.

## Audit Command

Run the checked-in audit against a live Canary instance:

```bash
bin/dogfood-audit --strict
```

The command reads `CANARY_ENDPOINT` and `CANARY_API_KEY`, compares the live
target set against [priv/dogfood/owned_services.json](/Users/phaedrus/Development/canary/priv/dogfood/owned_services.json),
and prints:

- the unified Canary report summary for the requested window
- every active owned HTTP service with target presence, URL match, health state,
  and current error totals
- pending services whose public health surfaces are not yet verified from the
  operator environment
- follow-on services that are intentionally out of scope for HTTP dogfooding

Use `--window 1h` or another supported window when you want a tighter read.

## Active Dogfood Set

The 2026-04-17 audit verified these active owned HTTP services in live Canary:

| Service | Target URL | Live state |
|---------|------------|------------|
| `chrondle` | `https://www.chrondle.app/api/health` | `up` |
| `linejam` | `https://www.linejam.app/api/health` | `up` |
| `volume` | `https://www.volume.fitness/api/health` | `up` |
| `vulcan` | `https://adminifi-vulcan-orchestrator.fly.dev/health` | `up` |

Observed live summary on 2026-04-17 with `window=24h`:

- Canary report: `5 targets monitored. 433 errors across 1 service in the last 24 hours.`
- Active owned HTTP services all had matching target URLs.
- `chrondle` showed live error traffic (`433` `TypeError` events in the last 24h).
- `linejam`, `volume`, and `vulcan` reported `0` errors in the same window.

`canary-self` remains a valid extra target, but it is not part of the owned
service dogfood manifest.

## Pending / Follow-On Inventory

The old backlog note overstated the live set. As of the 2026-04-17 audit:

- `adminifi-web` is **not** in Canary's live target set. The Azure origin
  `https://apollo-app-service.azurewebsites.net/health` resolved, but `/health`
  returned `404`, and the public `adminifi.app` host timed out from the
  operator environment.
- `consumer-portal` is **not** in Canary's live target set. Its repo documents
  `https://my-public-adminifi.azurewebsites.net/api/health`, but that hostname
  did not resolve from the operator environment.
- `time-tracker` is intentionally out of scope for this item because it is a
  desktop app. That work stays with backlog item `009`.
- `cerberus` has a Canary sink implementation, but this audit did not pin a
  canonical public HTTP health surface yet.

Keep the manifest current whenever an owned service is added, removed, or
reclassified. The audit command is the CLI-first source of truth for this wave.
