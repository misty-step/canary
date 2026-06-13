# Build a universal integration and enrollment engine

Priority: P0
Status: ready
Estimate: XL

## Goal
Make Canary integration frictionless for arbitrary apps by turning discovery, enrollment, platform setup, SDK/adapters, static-site coverage, and verification receipts into one state-aware loop.

## Oracle
- [ ] `bin/canary integrate status --json` merges local scan, integration manifest, platform env names, live Canary targets/monitors, query readback, webhook state, and dogfood registry evidence into one authoritative coverage verdict.
- [ ] The planner recognizes valid bespoke integrations, generated SDK integrations, static/Vercel sites, `src/app/**/api/health/route.ts`, app-route groups, non-HTTP monitors, and already-enrolled live targets without recommending duplicate patches.
- [ ] Static sites can be covered through a no-code or low-code path: health target enrollment, optional generated Vercel function/static health artifact, optional tiny browser capture snippet, and query/readback verification.
- [ ] Next.js adapter mode exports first-class server request capture, browser `error`/`unhandledrejection` observer, global/error-boundary helpers, Sentry dual-write bridge, health helper, and safe defaults.
- [ ] Non-HTTP runtimes get monitor/check-in templates for cron jobs, workers, desktop apps, and CLIs.
- [ ] Every successful integration writes or updates a reviewable `.canary/integration.json` receipt with service, environment, health URL, target/monitor IDs, webhook IDs, env names, verification commands, and last verified timestamp.
- [ ] `bin/dogfood-inventory --strict` consumes these receipts and expires stale coverage evidence instead of relying on prose next actions.

## Children
1. Add the coverage-state model and `integrate status` command.
2. Fix discovery for app groups, `src/app` layouts, bespoke Canary capture, existing live targets, and platform state.
3. Add static/Vercel no-code enrollment mode.
4. Promote LineJam/Chrondle patterns into `@canary-obs/sdk/nextjs`.
5. Add Sentry bridge/migration helpers for Sentry-heavy apps.
6. Add non-HTTP monitor/check-in templates and verification.
7. Generate `.canary/integration.json` receipts and make inventory read them.

## Notes
- Evidence: `docs/agent-inspection-cli.md` defines the current `discover/plan/patch/enroll` loop, but `crates/canary-cli/src/lib.rs` currently detects Canary code only by `@canary-obs/sdk` or `initCanary` strings and only checks fixed health paths.
- Live checks in this groom showed LineJam and Chrondle have valid Canary code that current `integrate discover` missed; Chrondle's health route under `src/app/(app)/api/health/route.ts` was also missed.
- Product lane recommended a universal enrollment engine as the main bridge from dogfood-optimized substrate to arbitrary-app product.
