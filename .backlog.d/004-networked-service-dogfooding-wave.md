# Networked Service Dogfooding Wave

Priority: medium
Status: ready
Estimate: L

## Goal
Bring the owned HTTP services `vulcan`, `consumer-portal`, and `web` under Canary using safe server-side integrations that prove the product on real workloads.

## Non-Goals
- Do not include `time-tracker` or any desktop-specific health model in this item
- Do not expose `CANARY_API_KEY` to browser bundles
- Do not require new SDK packages when direct HTTP or existing seams are sufficient

## Oracle
- [ ] Given `vulcan`, `consumer-portal`, and `web` are integrated, when Canary polls their health surfaces, then all three appear as targets under the correct service names
- [ ] Given each integrated service hits an error path, when Canary ingest is queried, then the error arrives under the expected service name without browser-held secrets
- [ ] Given the integration lane is complete, when the operational notes are reviewed, then the health URLs, reporting seams, and verification commands for the three networked services are documented

## Notes
This is the narrow execution slice of GitHub #68. The original issue mixed three HTTP services with one desktop app that cannot share the same health model.
