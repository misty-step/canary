# Codify a cold-agent readiness proof

Priority: P1 · Status: pending · Estimate: M

## Goal
Give a cold agent one discoverable Canary verification path that proves it can inspect, operate, and hand off evidence without re-deriving the repo runbook from scattered docs.

## Oracle
- [ ] Given a clean checkout with configured read/admin credentials, when a cold agent follows the readiness proof, then it runs `bin/canary doctor --json`, `bin/canary mcp-manifest`, the integration/status discovery smoke, and `./bin/validate --fast`, then writes a redacted receipt.
- [ ] Given credentials or GitHub/Fly access are missing, then the proof reports a concrete blocked field and replacement command without printing secret values.
- [ ] Given the MCP wrapper from #049 exists, then the readiness proof installs or launches it and invokes one no-op/read-only Canary tool through the wrapper.
- [ ] Given the proof is stale, then the repo gate or a shell test fails on missing command names, stale manifest tools, or absent receipt fields.

## Verification System
- Claim: a cold agent can discover and verify Canary's agent-facing operating surface from one repo-local entrypoint.
- Falsifier: an agent must read several docs to know what to run, the proof omits MCP/CLI/API evidence, or missing credentials look like generic failure.
- Driver: a shell test or harness script plus the generated readiness receipt.
- Grader: receipt contains command versions, doctor verdict, MCP tool count, integration/status result, validation result, redaction check, and explicit blocked/unavailable fields.
- Evidence packet: checked-in fixture receipt plus live receipt path from the implementation run.
- Cadence: fast gate for fixture shape; manual/live run before merging readiness changes.

## Notes
Why: the agent-readiness lane found that Canary has strong CLI JSON envelopes,
OpenAPI guidance, and an MCP manifest, but no single repo-local verification
entrypoint or skill-style artifact for a cold agent. This is smaller than #049:
#049 ships the installable MCP wrapper and integration proof; this ticket makes
the repo's own agent operating proof discoverable and repeatable.

## Children
1. Decide whether the entrypoint is a repo-local skill, script, or `bin/canary self-check`.
2. Define the redacted readiness receipt schema.
3. Add fixture tests for missing credentials and stale manifest/tool names.
4. Integrate the MCP wrapper smoke after #049 lands it.
5. Link the proof from `AGENTS.md`, `README.md`, and `docs/agent-inspection-cli.md`.
