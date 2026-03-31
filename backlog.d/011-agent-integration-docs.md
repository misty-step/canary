# Document the agent integration pattern

Priority: medium
Status: ready
Estimate: S

## Goal
Document the webhook-as-notification + timeline-as-replay pattern as the canonical agent integration model, so agent builders have a clear reference.

## Non-Goals
- Full OpenAPI spec generation (separate effort)
- Tutorials or walkthroughs — this is reference documentation

## Oracle
- [ ] Given the project docs (README, CLAUDE.md, or a dedicated API guide), when an agent developer reads them, then the polling pattern is described: webhook fires → agent wakes → polls timeline from last cursor → processes events → annotates
- [ ] Given the docs, when the crash-recovery pattern is described, then it covers: poll from last persisted cursor → catch up → resume
- [ ] Given the docs, when the `event_type` filter and `after` param are mentioned, then their usage is shown with concrete curl/httpie examples
- [ ] Given the docs, when the `after` vs `cursor` param precedence is described, then it states that `after` takes precedence when both are present

## Notes
Deferred from 002-timeline-agent-polling.md — the code is shipped but the pattern is undocumented.

The key insight: webhooks are fire-and-forget notifications. Timeline is the durable, queryable event log. Agents should never depend solely on webhook delivery for correctness.

Feeds into: 010-ramp-pattern.md (the ramp pattern's polling loop needs this documented).
