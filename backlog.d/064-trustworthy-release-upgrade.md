# Make releases and upgrades trustworthy

Priority: P1 · Status: pending · Estimate: L

## Goal
Give Canary a truthful release and upgrade story: published artifacts, accurate
release notes, working SDK install instructions, and a documented upgrade path
for self-hosted operators.

## Oracle
- [ ] Given a GitHub release is cut, then it includes truthful Rust-era release
      notes and release assets or links that match the supported install path.
- [ ] Given an operator wants to upgrade, then docs explain the supported path
      from an existing SQLite/Litestream deployment and the rollback boundary.
- [ ] Given `clients/typescript/INTEGRATION.md` says `npm install
      @canary-obs/sdk`, then `npm view @canary-obs/sdk` resolves with the
      expected subpath exports or the claim is removed from public docs.
- [ ] Given Landmark generates release intelligence for Canary, then false
      Elixir/Phoenix history does not reappear in release notes.
- [ ] Given a breaking contract change is proposed, then the policy says how
      OpenAPI, CLI, MCP, SDK, and migration compatibility are versioned.

## Verification System
- Claim: a self-hosted operator can install and upgrade Canary from published,
  truthful artifacts.
- Falsifier: release notes advertise dead surfaces, npm install 404s, upgrade
  docs require source archaeology, or release assets are absent without a
  documented replacement.
- Driver: release workflow checks, npm view smoke, docs grep, Landmark output
  review, and a local upgrade rehearsal where practical.
- Grader: artifacts exist or claims are scoped; docs commands resolve; release
  body matches current product surfaces.
- Evidence packet: release/upgrade transcript plus Landmark bug or fixture if
  the classifier caused the false history.

## Notes
This epic absorbs `051-typescript-sdk-npm-publish.md` and the groom report's
release/upgrade findings. It does not force a particular artifact strategy in
this backlog write; the implementation should choose the smallest truthful path
for the current product.

## Children
1. Publish the TypeScript SDK or remove the public npm-install claim until it is
   true.
2. Publish a Docker image or document why Fly remote builds remain the supported
   path for now.
3. Fix the v1.0.0 release-note truth gap and file an upstream Landmark
   regression fixture for Canary's false Elixir history.
4. Add self-host upgrade and rollback guidance.
5. Add a compatibility policy for OpenAPI, CLI, MCP, SDK, migrations, and
   webhook payloads.
