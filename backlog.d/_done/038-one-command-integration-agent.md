# Build a one-command Canary integration agent

Priority: high
Status: done
Estimate: XL

## Goal
Make it routine for an agent to discover, patch, enroll, and verify Canary coverage for a deployed app without bespoke hand wiring.

## Oracle
- [ ] `canary integrate discover <path-or-project>` detects framework, platform, deploy target, health route, existing Sentry usage, Canary code paths, and env-name presence without reading secret values.
- [ ] `canary integrate plan` emits a patch/enrollment plan that distinguishes SDK instrumentation, health route, env vars, Canary targets, monitors, webhooks, and optional platform drains.
- [ ] For Next.js apps, the patcher can add or verify `@canary-obs/sdk`, `instrumentation.ts` request error capture, browser/global error boundary capture, a simple health route, CSP/connect-src updates when needed, and tests.
- [ ] `canary integrate enroll` creates scoped Canary keys, targets, monitors, and webhook subscriptions through the Canary admin API, reading secret values only from stdin or an approved secret-manager handoff and redacting receipts.
- [ ] Vercel support can audit and configure env names for production and preview via the Vercel CLI; Fly support can audit app status and health URLs via `flyctl`.
- [ ] The first pilot lands on one currently missing app (`vanity` or `misty-step`) and one simple splash page, with deployed smoke proof and query readback.
- [ ] `./bin/validate --fast` is green, and docs explain when to choose SDK instrumentation versus platform-level drains.

## Notes
**Why:** Integration-friction perspective. Manual SDK snippets are acceptable for humans, but the better Canary product is an agent that makes every error path route to Canary and proves that it did.

**Creative path.** For Vercel apps, support both code instrumentation and platform-level capture where available. Vercel documents Log Drains for forwarding observability data to custom HTTP endpoints on supported plans, and the Vercel env CLI gives a current secret-name audit path. Drains are not a replacement for typed app context, but they can reduce missed server-side log/error paths.

**Boundaries.**
- No secret values in git, logs, or receipts.
- No blind codemods without a reviewable plan.
- No claiming coverage until a deployed smoke reads the service back from Canary.
