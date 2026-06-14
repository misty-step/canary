# Build a universal integration and enrollment engine

Priority: P0
Status: done
Estimate: XL

## Goal
Make Canary integration frictionless for arbitrary apps by turning discovery, enrollment, platform setup, SDK/adapters, static-site coverage, and verification receipts into one state-aware loop.

## Oracle
- [x] `bin/canary integrate status --json` merges local scan, integration manifest, platform env names, live Canary targets/monitors, query readback, webhook state, and dogfood registry evidence into one authoritative coverage verdict.
- [x] The planner recognizes valid bespoke integrations, generated SDK integrations, static/Vercel sites, `src/app/**/api/health/route.ts`, app-route groups, non-HTTP monitors, and already-enrolled live targets without recommending duplicate patches.
- [x] Static sites can be covered through a no-code or low-code path: health target enrollment, optional generated Vercel function/static health artifact, optional tiny browser capture snippet, and query/readback verification.
- [x] Next.js adapter mode exports first-class server request capture, browser `error`/`unhandledrejection` observer, global/error-boundary helpers, Sentry dual-write bridge, health helper, and safe defaults.
- [x] Non-HTTP runtimes get monitor/check-in templates for cron jobs, workers, desktop apps, and CLIs.
- [x] Every successful integration writes or updates a reviewable `.canary/integration.json` receipt with service, environment, health URL, target/monitor IDs, webhook IDs, env names, verification commands, and last verified timestamp.
- [x] `bin/dogfood-inventory --strict` consumes these receipts and expires stale coverage evidence instead of relying on prose next actions.

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

## Completion

Delivered on 2026-06-14.

- `bin/canary integrate status` now reconciles local discovery, integration
  receipts, platform env-name evidence, live targets, monitors, webhooks, query
  readback, and dogfood inventory into one coverage verdict.
- Discovery now recognizes route-grouped Next.js health endpoints, bespoke
  Canary capture paths, static-site coverage mode, Vercel env names without
  secret values, non-HTTP monitor templates, and already-enrolled live
  target/monitor state.
- `integrate patch` and `integrate enroll --project-root` write
  `.canary/integration.json` receipts. Planned receipts stay non-covering;
  verified receipts require live target or monitor evidence before dogfood
  inventory counts them as coverage.
- The TypeScript SDK now includes check-ins plus Next.js helpers for browser
  observers, global/error-boundary capture, Sentry dual-write, and health
  responses.
- The MCP tool manifest and integration docs expose the new status and
  receipt-backed enrollment loop.

Evidence:

- `cargo test -p canary-cli --locked`
- `cargo clippy -p canary-cli --all-targets --locked -- -D warnings`
- `bash test/bin/dogfood_inventory_test.sh`
- `shellcheck bin/dogfood-inventory`
- `npm --prefix clients/typescript run test:ci`
- `npm --prefix clients/typescript run typecheck`
- `npm --prefix clients/typescript run build`
- `bin/canary integrate discover /Users/phaedrus/Development/vanity --service vanity --production-url https://www.phaedrus.io --json`
- `bin/canary integrate status . --service canary --production-url https://canary-obs.fly.dev --json`
- `bin/canary integrate plan . --service canary --production-url https://canary-obs.fly.dev --json`
- `diff -u priv/mcp/canary-cli-tools.json <(bin/canary mcp-manifest)`
- `git diff --check`
- `./bin/validate --fast`
- `./bin/validate --strict`
