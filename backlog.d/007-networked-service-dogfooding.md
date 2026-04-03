# Networked service dogfooding wave

Priority: medium
Status: ready
Estimate: L

## Goal
Bring owned HTTP services under Canary using safe server-side integrations that prove the product on real workloads.

## Non-Goals
- Include desktop apps or non-HTTP health models in this item
- Expose `CANARY_API_KEY` to browser bundles
- Require new SDK packages when direct HTTP or existing seams are sufficient

## Oracle
- [ ] Given target services are integrated, when Canary polls their health surfaces, then all appear as targets under the correct service names
- [ ] Given each integrated service hits an error path, when Canary ingest is queried, then the error arrives under the expected service name
- [ ] Given the integration is complete, when operational notes are reviewed, then health URLs, reporting seams, and verification commands are documented

## Notes
Real workload validation. Without dogfooding, annotations and timeline enrichment are designed in a vacuum.

As of 2026-04-01 audit: 7 consumer services are already integrated (linejam, chrondle,
volume, vulcan, adminifi-web, consumer-portal, time-tracker). Three Vercel apps had
env vars configured ~2026-03-26. Cerberus has a full Python sink at
`pkg/canary.py` with `canary.enabled: false` — a free dogfooding target (config flip).

Oracle should be updated to reflect that some services are already reporting.
Migrated from .backlog.d/004.
