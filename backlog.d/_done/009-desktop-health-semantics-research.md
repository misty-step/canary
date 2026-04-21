# Desktop health semantics research

Priority: low
Status: done
Estimate: M

## Goal
Choose the canonical Canary health model for non-HTTP runtimes such as desktop apps, cron jobs, and workers.

## Non-Goals
- Implement the chosen model across external repos in this item
- Pretend desktop apps can use the same polling semantics as HTTP services
- Expand Canary into generic job orchestration

## Oracle
- [x] Given the research completes, when the output is reviewed, then it compares heartbeat, hosted relay, local companion, and crash-only reporting approaches
- [x] Given a decision is made, when the item closes, then one canonical API surface for non-HTTP health signals is proposed
- [x] Given the decision is made, when the item closes, then a follow-up implementation item is created or the desktop-health lane is explicitly rejected

## Notes
Closed on 2026-04-17 with the decision document in
`docs/non-http-health-semantics.md`.

Selected model:

- Add non-HTTP check-in monitors as a new entity, separate from URL-backed targets.
- Add one canonical writer endpoint: `POST /api/v1/check-ins`.
- Keep `POST /api/v1/errors` for actual crashes and exceptions.
- Reuse existing `health_check.*` timeline / webhook semantics for monitor state transitions.

Implementation follow-up: `021`.

Migrated from .backlog.d/009.
