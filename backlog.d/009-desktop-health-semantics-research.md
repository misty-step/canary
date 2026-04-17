# Desktop health semantics research

Priority: low
Status: ready
Estimate: M

## Goal
Choose the canonical Canary health model for non-HTTP runtimes such as desktop apps, cron jobs, and workers.

## Non-Goals
- Implement the chosen model across external repos in this item
- Pretend desktop apps can use the same polling semantics as HTTP services
- Expand Canary into generic job orchestration

## Oracle
- [ ] Given the research completes, when the output is reviewed, then it compares heartbeat, hosted relay, local companion, and crash-only reporting approaches
- [ ] Given a decision is made, when the item closes, then one canonical API surface for non-HTTP health signals is proposed
- [ ] Given the decision is made, when the item closes, then a follow-up implementation item is created or the desktop-health lane is explicitly rejected

## Notes
Unblocked on 2026-04-17 when `007` closed with a verified live HTTP dogfood set.
Migrated from .backlog.d/009.
