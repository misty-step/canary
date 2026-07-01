# Plumb the Factory fleet into Canary

Priority: P0 · Status: pending · Estimate: L

## Goal
Make every Factory app report errors, health, and uptime to the Misty Step
Canary instance through a repeatable 15-minute integration path per app.

## Oracle
- [ ] Given each active Factory repo, then the repo has an explicit Canary
      coverage status: integrated, intentionally deferred, or blocked with the
      missing credential/surface named.
- [ ] Given an integrated app, then live Canary readback proves at least one
      health/uptime signal and one error or check-in path for that service.
- [ ] Given an agent starts from a Factory repo, then it can find the 15-minute
      integration path without rediscovering Canary's API, SDK, CLI, or MCP
      surface.
- [ ] Given integration fails, then the receipt records the concrete blocker
      without logging secret values.
- [ ] Given dogfood audit runs in strict mode, then stale or registry-only
      coverage cannot pass as verified fleet coverage.

## Verification System
- Claim: Canary is the monitoring half of the Factory composition, not just a
  standalone health service.
- Falsifier: a repo is counted as covered without live readback, integration
  requires bespoke handwork, or stale dogfood registry rows pass strict mode.
- Driver: `bin/canary integrate status`, `bin/dogfood-audit --strict --json`,
  service-specific query/timeline readback, and per-repo integration receipts.
- Grader: each service row has fresh evidence timestamps, exact endpoint/service
  identity, and a clear next action.
- Evidence packet: fleet coverage matrix plus redacted integration receipts.

## Notes
This is the composition-facing follow-up after the cold-operator path. It
reuses the existing integration engine and the future evidence hardening in
`049`; it should not turn Canary into a repo mutation engine. Downstream app
patches happen in those repos or through external agents.

## Progress
- 2026-07-01: Bastion lane verified `canary-obs.internal` resolution and route
  path from the fleet, but connections to `canary-obs.internal:4000` were
  refused because the production server was not listening on the Fly 6PN-facing
  address family. Added a server bind fix to listen on `[::]:PORT`; deployment
  proof must include a curl from outside the Fly machine.

## Children
1. Define the active Factory app inventory and coverage statuses.
2. Publish a 15-minute integration recipe that works through API, CLI, MCP, or
   SDK depending on the app shape.
3. Require service-specific health/uptime readback for every integrated app.
4. Require service-specific error/check-in or synthetic signal readback where
   the app surface supports it.
5. Tighten dogfood strictness so stale registry-only coverage cannot satisfy
   fleet coverage.
6. Feed missing SDK/MCP/browser-capture gaps into `049` rather than duplicating
   them here.
