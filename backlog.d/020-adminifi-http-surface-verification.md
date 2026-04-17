# Adminifi HTTP surface verification

Priority: low
Status: blocked
Estimate: S

## Goal
Pin canonical public health URLs for `adminifi-web` and `consumer-portal`, then
onboard those HTTP surfaces into Canary under the correct service names.

## Non-Goals
- Rebuild or redeploy Adminifi services from this repo
- Treat private origins or undocumented URLs as canonical production surfaces
- Pull desktop or non-HTTP runtimes back into the HTTP dogfooding lane

## Oracle
- [ ] Given canonical public health URLs are published and resolvable, when `bin/dogfood-audit --strict` is rerun after onboarding, then `adminifi-web` and `consumer-portal` appear as live targets under the expected service names
- [ ] Given each service emits a natural error or explicit Canary verification event, when the service query API is checked, then errors land under the expected service name
- [ ] Given the follow-on closes, when the dogfood docs and manifest are reviewed, then the pending-service section is removed or updated with the shipped URLs

## Notes
Spun out of `007` on 2026-04-17 once the active HTTP dogfood set was verified.

Blocking evidence from the 2026-04-17 operator audit:

- `https://apollo-app-service.azurewebsites.net/health` resolved, but returned `404`
- `https://adminifi.app` timed out from the operator environment
- `https://my-public-adminifi.azurewebsites.net/api/health` did not resolve
