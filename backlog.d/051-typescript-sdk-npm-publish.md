# Publish the TypeScript SDK (@canary-obs/sdk) to npm

Priority: P1 · Status: pending · Estimate: S

## Goal
Make the documented `npm install @canary-obs/sdk` integration path actually work, so external consumers (e.g. Adminifi Habitat) can adopt the Next.js SDK without git/`file:` workarounds.

## Why now
Habitat-dogfooding surfaced this. `clients/typescript/INTEGRATION.md` instructs `npm install @canary-obs/sdk` (incl. the `@canary-obs/sdk/nextjs` subpath), but the package is **not published** — `npm view @canary-obs/sdk` returns 404 and no workflow runs `npm publish`. The SDK exists, is built (committed `dist/`), and is tested (`clients/typescript/test/nextjs.test.ts`), so this is a **release-pipeline gap, not a build gap**. Today an external consumer must POST the raw HTTP ingest contract (`/api/v1/errors`, `/api/v1/check-ins`) or vendor the SDK — friction that blocks the advertised onboarding.

## Oracle
- [ ] `npm view @canary-obs/sdk` resolves a published version, with the `@canary-obs/sdk/nextjs` subpath export intact.
- [ ] A CI workflow publishes on tag/release (semver bump + provenance), gated on the existing TS test suite.
- [ ] `INTEGRATION.md`'s `npm install` path works from a clean external project (Next.js).
- [ ] The package leaves the `0.1.0` placeholder for a real release version.

## Relationship to existing backlog
Net-new — no existing item covers SDK release. Complements #049 (which ships the MCP wrapper + browser-capture helpers but does not address npm publication). Unblocks the polished integration path; the raw-HTTP fallback remains available regardless.
