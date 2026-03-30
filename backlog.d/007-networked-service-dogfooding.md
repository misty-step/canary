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
Migrated from .backlog.d/004.
