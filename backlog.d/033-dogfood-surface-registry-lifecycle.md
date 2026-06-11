# Turn dogfood services into a live evidence registry

Priority: medium
Status: ready
Estimate: M

## Goal
Replace static owned-service dogfood buckets with a schema-driven registry that records current state, evidence age, failure mode, owner, and next action for every service Canary should monitor.

## Oracle
- [ ] `priv/dogfood/owned_services.json` has explicit per-service state (`active`, `pending`, `follow_on`, or `ignored`) plus required evidence fields such as `last_checked_at`, `failure_mode`, `owner`, and `next_action`.
- [ ] `bin/dogfood-audit --strict` validates the registry shape, emits a machine-readable report, and keeps pending/follow-on services visible without failing live owned-service checks.
- [ ] `docs/networked-service-dogfooding.md` is regenerated from or mechanically reconciled with the registry so the April 2026 prose snapshot cannot drift silently.
- [ ] `adminifi-web` and `consumer-portal` carry the latest canonical URL evidence in the registry; backlog item `020` either points at this lifecycle or is merged after user ratification.
- [ ] `./bin/validate --fast` includes a deterministic schema/audit fixture test that does not require live network access.

## Notes
**Why:** Product dogfood perspective. The current audit doc is an April 17 snapshot, and `owned_services.json` separates active, pending, and follow-on services without timestamps or next actions. That makes blocked services like Adminifi visible but not actively managed.

**Children**
1. Define the registry schema and migrate the current active/pending/follow-on data without changing live targets.
2. Teach `bin/dogfood-audit` to validate state/evidence fields and emit structured report output.
3. Reconcile `020-adminifi-http-surface-verification.md` with the registry lifecycle after the user ratifies whether it remains a standalone blocked ticket.

**Responder-boundary check.** This manages Canary's owned-service monitoring evidence only; it does not redeploy or mutate downstream services.
