# Record durable read-audit events for responder context fetches

Priority: P1 · Status: ready · Estimate: M

## Goal
Every time a responder-scoped (non-admin) key reads rich context — `GET
/api/v1/report` and any incident/error detail-by-id route — Canary durably
records who read what and when, so a future minimization/redaction pass (048)
has a real read trail to reason about instead of starting from zero.

## Oracle
- [ ] A responder-write-scoped or read-only-scoped request to `/api/v1/report`
      (route: `crates/canary-server/src/lib.rs:143`, gated by
      `require_read_scope`/`require_responder_write_scope` in
      `crates/canary-server/src/server_auth.rs:53`) writes one durable audit
      record capturing: key id, service binding, route, and timestamp.
- [ ] The audit record reuses the existing `service_events` table's
      `retention_class = 'audit'` lane (`crates/canary-store/src/schema.rs:680`)
      rather than introducing a new table, unless a concrete schema conflict is
      found and documented in the PR.
- [ ] Admin-key reads are exempt or separately labeled (admin already has full
      visibility; the goal is a responder-specific trail, not blanket request
      logging).
- [ ] Audit records never contain the actual redacted/minimized payload body —
      only route, scope, key id, service, and timestamp (no payload replay from
      the audit log itself).
- [ ] A test proves: one responder read produces exactly one audit event with
      the correct service binding; a cross-service or admin read does not
      pollute another service's audit trail.
- [ ] `./bin/validate` passes.

## Notes
This is the P0 "add durable read-audit events for rich responder context
fetches" line from `backlog.d/048-responder-rich-context-safety-gate.md`,
deliberately narrowed: it does NOT attempt the context-envelope redesign,
minimization/redaction policy, or new responder scopes that make 048 itself
XL and ADR-gated. Recording *that a read happened* is mechanical and doesn't
require deciding *what should be redacted* — that decision stays in 048.
`service_events.retention_class` already has an `'audit'` value carved out in
the schema check constraint but (as far as this investigation found) nothing
writes it yet — worth confirming before assuming it's unused.

**Why:** 048 is the single most important safety gate in the backlog (P0, XL)
but everything in it is currently one big ADR-shaped decision. Landing the
mechanical read-audit piece now gives 048's eventual redesign real audit data
to design against, and unblocks part of its Oracle without requiring the
scope-model judgment call tonight.
