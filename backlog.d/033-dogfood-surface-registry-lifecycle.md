# Turn deployed services into a live evidence registry

Priority: high
Status: ready
Estimate: M

## Goal
Replace static owned-service dogfood buckets with a schema-driven deployed-service registry that records current state, evidence age, platform, failure mode, owner, and next action for every service Canary should monitor or explicitly ignore.

## Oracle
- [ ] `priv/dogfood/owned_services.json` has explicit per-service state (`active`, `pending`, `blocked`, `follow_on`, `suspended`, or `ignored`) plus required evidence fields such as `platform`, `production_url`, `health_url`, `last_checked_at`, `failure_mode`, `owner`, and `next_action`.
- [ ] `bin/dogfood-audit --strict` validates the registry shape, emits a machine-readable report, and keeps pending/follow-on services visible without failing live owned-service checks.
- [ ] `docs/networked-service-dogfooding.md` is regenerated from or mechanically reconciled with the registry so the April 2026 prose snapshot cannot drift silently.
- [ ] `adminifi-web`, `consumer-portal`, `vanity`, `misty-step`, `sploot`, `trump-goggles-splash`, and `timeismoney-splash` carry the latest canonical URL and platform evidence in the registry; backlog item `020` either points at this lifecycle or is merged after user ratification.
- [ ] `./bin/validate --fast` includes a deterministic schema/audit fixture test that does not require live network access.

## Notes
**Why:** Product dogfood perspective. The current audit doc is an April 17 snapshot, and `owned_services.json` separates active, pending, and follow-on services without timestamps or next actions. That makes blocked services like Adminifi visible but not actively managed, and it misses newly requested Vercel/Fly deployments that should be covered.

**2026-06-11 evidence.** Vercel CLI showed `misty-step`, `vanity`, `sploot`,
`linejam`, `chrondle`, `timeismoney-splash`, and `trump-goggles-splash` under
the `misty-step` team. Fly showed deployed `canary-obs`,
`linejam-canary-responder`, `memory-engine-api`, and `vox-cloud-api`. The live
Canary audit still only treated `chrondle`, `linejam`, `volume`, and `vulcan`
as active owned HTTP services, with `canary-self` ignored as an extra target.

**Children**
1. Define the registry schema and migrate the current active/pending/follow-on data without changing live targets.
2. Teach `bin/dogfood-audit` to validate state/evidence fields and emit structured report output.
3. Reconcile `020-adminifi-http-surface-verification.md` with the registry lifecycle after the user ratifies whether it remains a standalone blocked ticket.
4. Import the requested Vercel/Fly app inventory as registry entries with explicit coverage state.

**Responder-boundary check.** This manages Canary's owned-service monitoring evidence only; it does not redeploy or mutate downstream services.
