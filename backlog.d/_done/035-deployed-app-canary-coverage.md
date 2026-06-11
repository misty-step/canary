# Make deployed applications exhaustively covered by Canary

Priority: high
Status: done
Estimate: XL

## Goal
Ensure every active owned deployment across Vercel, Fly, and local deployment metadata is enrolled in Canary for health monitoring, error ingest, and agent-readable status.

## Oracle
- [x] A deployed-surface inventory command enumerates Vercel scopes `misty-step` and `adminifi-growth`, Fly apps, checked-in local `.vercel/project.json` links, and the Canary registry, then emits JSON with `covered`, `partial`, `blocked`, or `ignored` for every active surface.
- [x] The registry includes at least `canary`, `vanity`, `chrondle`, `linejam`, `misty-step`, `trump-goggles-splash`, `timeismoney-splash`, and `sploot`, with owner, platform, production URL, repo path when known, health URL or monitor mode, ingest status, and last audit timestamp.
- [x] Each requested project either has a live Canary target or monitor plus query readback evidence, or a documented blocker with a next action and owner.
- [x] Vercel env-name audits verify Canary server/browser keys for production and preview where applicable without printing secret values.
- [x] `bin/dogfood-audit --strict --window 24h` is extended or paired with the new inventory command so stale manifest entries, missing active deployments, and extra live targets fail in strict mode.
- [x] `./bin/validate --fast` covers deterministic fixture tests for the inventory and registry logic.

## Notes
**Why:** User dogfood coverage request. Canary should be the primary uptime, health-check, and error-tracking substrate for all deployed owned apps, not just the four services from the April audit.

**2026-06-11 evidence.** Vercel CLI listed `misty-step`, `vanity`, `sploot`, `linejam`, `chrondle`, `timeismoney-splash`, and `trump-goggles-splash` under the `misty-step` scope. Fly listed `canary-obs`, `linejam-canary-responder`, `memory-engine-api`, and `vox-cloud-api` as deployed. The live Canary manifest currently covers `chrondle`, `linejam`, `volume`, and `vulcan`; `canary-self` exists only as an ignored extra target.

**Requested project gaps.**
- `vanity`: no Canary env names, no local Canary code hits, and no common health route returned 200.
- `misty-step`: Sentry-centric, no Canary env names, but `/api/health` returns 200.
- `trump-goggles-splash` and `timeismoney-splash`: Vercel projects exist, local dirs are not linked to Vercel, no Canary hits, and no common health route returned 200.
- `sploot`: Canary ingest exists and live errors appear as `sploot-web`, but it is missing from the checked-in dogfood manifest and active target set.
- `chrondle`: Canary is active, but current 24h signal had a `TypeError` flood that should become an incident or follow-up.

**Children**
1. Add a platform inventory reader for Vercel and Fly that records names and URLs without reading secret values.
2. Enroll or explicitly block the requested projects and record one evidence packet per service.
3. Make strict dogfood audit fail on missing requested coverage unless the registry carries a current blocker.

**Related.** #033 shipped the registry substrate; this item is the coverage outcome.

**Delivered by /deliver 2026-06-11.** Added `bin/dogfood-inventory` as the
agent-readable deployed-surface inventory. It joins the Canary registry with
live Vercel project inventory, live Fly app inventory, and local
`.vercel/project.json` links, emits JSON coverage classifications, and fails
strict mode on collector errors, missing active deployments, unregistered
deployments, unregistered local links, or absent requested services. The
registry now records platform project names, local repo paths, monitor mode,
and ingest status for the requested owned services. Live operator evidence on
2026-06-11 confirmed the installed Vercel CLI emits JSON for
`vercel project ls --scope <scope> --format json`; the live inventory surfaced
the requested covered/partial/blocked services plus residual unregistered
owned deployments for follow-up classification.
