# Human dashboard read contract and private-network mode

Priority: P2 · Status: pending · Estimate: M

## Goal
Make the dashboard read contract explicit enough that a read-only key can answer
the operator's support views without opportunistically probing admin endpoints,
and decide whether self-hosted private-network deployments should enable
keyless reads through an env-gated mode.

## Oracle
- [ ] Given a read-only key opens the Checks view, then exact target cadence and
      monitor configuration needed for the view are available through a
      read-scoped endpoint.
- [ ] Given the public `canary-obs` app serves the dashboard, then no
      unauthenticated route returns private data.
- [ ] Given a self-hosted operator explicitly enables keyless private-network
      dashboard reads, then the mode is guarded by configuration and documented
      as unsafe for public Fly apps.
- [ ] Given OpenAPI is inspected, then the dashboard's read dependencies and
      scopes are visible to agents.

## Verification System
- Claim: the dashboard has a deliberate least-privilege read story instead of
  relying on admin keys for polish.
- Falsifier: a read-only key cannot show configured watch cadence, or an env
  flag accidentally opens data on a public app.
- Driver: auth/scope route tests, OpenAPI scope checks, local dashboard QA with
  read-only and admin keys, and a public-app auth smoke.
- Grader: read-only dashboard works without admin-only fallbacks for its core
  support views; any keyless mode is impossible unless explicitly configured.
- Evidence packet: PR screenshots plus auth matrix transcript.

## Notes
The v1 dashboard chooses session-key paste because `canary-obs` is public. This
ticket is the place to make a separate private-network affordance explicit if
operators still want powder-style keyless reads on isolated deployments.
