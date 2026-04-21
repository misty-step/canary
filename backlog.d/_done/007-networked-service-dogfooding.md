# Networked service dogfooding wave

Priority: medium
Status: done
Estimate: L

## Goal
Bring owned HTTP services under Canary using safe server-side integrations that prove the product on real workloads.

## Non-Goals
- Include desktop apps or non-HTTP health models in this item
- Expose `CANARY_API_KEY` to browser bundles
- Require new SDK packages when direct HTTP or existing seams are sufficient

## Oracle
- [x] Given the active owned HTTP dogfood set (`chrondle`, `linejam`, `volume`, `vulcan`), when `bin/dogfood-audit --strict` runs against live Canary, then each appears under the correct service name and expected health URL
- [x] Given live service query endpoints are inspected through the same audit, when current traffic is reviewed, then each active service reports its current error total and summary under the expected service name
- [x] Given the integration is complete, when operational notes are reviewed, then health URLs, reporting seams, pending surfaces, and verification commands are documented in `docs/networked-service-dogfooding.md`

## Notes
Real workload validation. Without dogfooding, annotations and timeline enrichment are designed in a vacuum.

Closed on 2026-04-17 after the checked-in dogfood audit proved the active HTTP
set already live in Canary: `chrondle`, `linejam`, `volume`, and `vulcan`.
Live 24h audit result:

- `5 targets monitored. 433 errors across 1 service in the last 24 hours.`
- All four active owned HTTP services were `up` and matched the expected target URLs.
- `chrondle` showed live error traffic (`433` `TypeError` events in the last 24h).

The old note claiming seven integrated services was stale. `time-tracker` moved
back to the desktop/non-HTTP lane (`009`), and unresolved Adminifi public
health surfaces were split into `020`.

Migrated from .backlog.d/004.
