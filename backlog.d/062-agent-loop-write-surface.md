# Close the agent loop through CLI and MCP writeback

Priority: P0 · Status: pending · Estimate: XL

## Goal
Let an agent complete the full Canary responder loop through aligned HTTP, CLI,
and MCP surfaces: read bounded context, create or observe a claim, write an
annotation, release or verify the claim, and replay the evidence later with
least-privilege responder authority.

## Oracle
- [ ] Given an agent receives a Canary incident webhook, then it can use MCP
      alone to read context, claim the subject, annotate evidence, and release
      or verify the claim.
- [ ] Given a CLI user performs the same loop, then the JSON envelopes and
      errors match the HTTP authority model.
- [ ] Given a responder key is service-bound, then it can perform permitted
      claim/annotation actions for that service and cannot read or mutate
      another service.
- [ ] Given MCP manifests are generated, then scopes and tool descriptions name
      write authority accurately.
- [ ] Given rich context is fetched, then responder context minimization and
      read-audit requirements from `048` are satisfied.

## Verification System
- Claim: the agent-facing loop is closable without raw route trivia or admin
  keys.
- Falsifier: an agent must fall back to raw HTTP for annotations, use admin
  authority for responder writes, or receives different semantics across HTTP,
  CLI, and MCP.
- Driver: route authorization tests, CLI JSON tests, MCP manifest parity tests,
  and a responder-loop conformance receipt.
- Grader: least-privilege failures return RFC 9457 Problem Details; successful
  loop leaves claim and annotation evidence visible in report/timeline.
- Evidence packet: checked-in conformance transcript under `docs/architecture/`.

## Notes
This epic adopts existing `048` as a child safety gate and adds the operator
decision that annotation write must be available through CLI/MCP. The older
Canary vision was correct about readback and claims; this makes the write side
operational for agents.

2026-07-04 incident slice: this PR completes the incident loop end to end
through CLI and MCP: incident list/detail, collision-safe remediation claim,
annotation evidence writeback, claim release or terminal verification, and
timeline replay of claim plus annotation writes with actor identity. It does not
complete `048` rich-context redaction/read-audit requirements, safe browser
capture, or monitor/check-specific write ergonomics. Those remain explicit
follow-ups rather than being smuggled into the incident loop.

## Children
1. Finish `048-responder-rich-context-safety-gate.md` or the narrow part needed
   for scoped responder keys.
2. Add CLI annotation create/list helpers for supported durable subjects.
3. Expose annotation write and claim lifecycle tools through the MCP server.
4. Add incident/error detail-by-id tools where MCP currently forces route
   trivia.
5. Align OpenAPI agent guidance, CLI help, and MCP manifests around one loop.
6. Add responder-loop conformance fixtures and a redacted receipt.
7. Add monitor/check-specific write ergonomics only after the incident loop and
   048 safety boundaries are reviewed.
