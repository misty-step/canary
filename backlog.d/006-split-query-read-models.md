# Split Query into domain read models

Priority: high
Status: done
Estimate: L

## Goal
Reduce `Canary.Query` coupling by separating error, health, incident, and search read concerns behind narrower interfaces without changing external API behavior.

## Non-Goals
- Change `/api/v1/report`, `/api/v1/query`, or `/api/v1/status` response shapes
- Rewrite the report endpoint from scratch
- Fold health-check orchestration changes into this item

## Oracle
- [x] Given the refactor is complete, when the codebase is inspected, then `lib/canary/query.ex` no longer acts as the single read-model nexus and no module exceeds the 500 LOC quality bar
- [x] Given the report, query, and status endpoints already work, when their tests run after the refactor, then behavior remains unchanged
- [x] Given the new read-model split exists, when a maintainer opens the code, then health, search, incidents, and error querying live behind distinct module boundaries
- [x] Given `mix test` runs, then the read-path regression suite is green

## Notes
`lib/canary/query.ex` is 620 LOC as of 2026-04-01 (was 523 at backlog creation) — the
only module above the 500 LOC quality bar and growing. Codex flagged this as
under-prioritized during the architecture audit. Splitting enables cleaner
annotation-aware queries (001) and agent-specific read paths.
Migrated from .backlog.d/005.

## What Was Built

Shipped 2026-04-14 on branch `refactor/query-read-models`.

Final layout:
- `lib/canary/query.ex` — thin facade: `defdelegate` public entrypoints, `search/2` window-adapter, `report_slice/1` cross-domain composition.
- `lib/canary/query/errors.ex` — owns `errors_by_service/3`, `errors_by_error_class/3`, `errors_by_class/1`, `error_detail/1`, `error_groups/1`, `error_summary/1` plus all errors-domain private helpers.
- `lib/canary/query/health.ex` — owns `health_targets/0`, `health_status/0`, `target_checks/2`, `recent_transitions/1`, plus `fetch_recent_checks/2`.
- `lib/canary/query/incidents.ex` — owns `active_incidents/1` and all incident-filtering + formatting helpers.
- `lib/canary/query/search.ex` and `window.ex` — unchanged.

Design decisions:
- **Thin `defdelegate` facade retained** rather than migrating existing callers. `Canary.Query` stays as the stable public entrypoint, and `report_slice/1` remains the only cross-domain composition point.
- **No `Canary.Query.Shared` module.** Every private helper has a single-domain caller set; a Shared module would have been a shallow pass-through by Ousterhout's test.
- **`report_slice/1` pins one reference time.** It captures `DateTime.utc_now()` once and passes it through the domain modules via `at:` so error groups, summaries, recent transitions, and incidents all share the same window boundary.

Verification:
- No file exceeds the 500 LOC quality bar.
- Report, incident, and query behavior remains covered through the existing regression suite.
- Internal read-model helpers stay behind their owning domain modules; callers continue to use `Canary.Query`.
