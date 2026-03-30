# Split Query into domain read models

Priority: medium
Status: ready
Estimate: L

## Goal
Reduce `Canary.Query` coupling by separating error, health, incident, and search read concerns behind narrower interfaces without changing external API behavior.

## Non-Goals
- Change `/api/v1/report`, `/api/v1/query`, or `/api/v1/status` response shapes
- Rewrite the report endpoint from scratch
- Fold health-check orchestration changes into this item

## Oracle
- [ ] Given the refactor is complete, when the codebase is inspected, then `lib/canary/query.ex` no longer acts as the single read-model nexus and no module exceeds the 500 LOC quality bar
- [ ] Given the report, query, and status endpoints already work, when their tests run after the refactor, then behavior remains unchanged
- [ ] Given the new read-model split exists, when a maintainer opens the code, then health, search, incidents, and error querying live behind distinct module boundaries
- [ ] Given `mix test` runs, then the read-path regression suite is green

## Notes
`lib/canary/query.ex` is 523 LOC — the only module above the 500 LOC quality bar. Splitting this enables cleaner annotation-aware queries (001) and agent-specific read paths.
Migrated from .backlog.d/005.
