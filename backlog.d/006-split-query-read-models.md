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

Shipped 2026-04-14 on branch `refactor/query-read-models` (4 commits: 573cb1b → 933cb31).

Final layout:
- `lib/canary/query.ex` — 48-LOC thin facade: 11 `defdelegate` lines, `search/2` window-adapter, `report_slice/1` cross-domain composition.
- `lib/canary/query/errors.ex` — 315 LOC. Owns `errors_by_service/3`, `errors_by_error_class/3`, `errors_by_class/1`, `error_detail/1`, `error_groups/1`, `error_summary/1` plus all errors-domain private helpers (cursor, annotation filter, classification select, format_group, build_error_detail).
- `lib/canary/query/health.ex` — 126 LOC. Owns `health_targets/0`, `health_status/0`, `target_checks/2`, `recent_transitions/1`, plus `fetch_recent_checks/2`.
- `lib/canary/query/incidents.ex` — 153 LOC. Owns `active_incidents/1` and all incident-filtering + formatting helpers.
- `lib/canary/query/search.ex` and `window.ex` — unchanged.

Design decisions:
- **Thin `defdelegate` facade retained** rather than migrating 27 caller sites. Precedent: `Canary.Query.search/2` was already a facade. Facade is genuinely deep because `report_slice/1` composes across all three domain modules — not a shallow pass-through.
- **No `Canary.Query.Shared` module.** Every private helper has a single-domain caller set; a Shared module would have been a shallow pass-through by Ousterhout's test.
- **Domains own cutoff resolution.** Ousterhout critic flagged the initial `@doc false *_since/1` helpers as information leakage. Fixed in 933cb31: `report_slice/1` now calls the public `error_groups(window)`, `error_summary(window)`, `recent_transitions(window)` with the raw window string and unwraps via `with`. Three redundant `Window.to_cutoff` calls (~microseconds) buy a cleaner interface and preserved "cutoff is internal" invariant.

Verification (all green):
- `mix test`: 337 tests, 0 failures (baseline and post-refactor identical).
- `mix format --check-formatted`: clean.
- `mix compile --warnings-as-errors`: clean.
- `./bin/validate --strict`: pass.
- No file exceeds 500 LOC quality bar.
- Zero callers outside the query tree reference any internal helper name.

Workarounds: none. Clean pure-move refactor; no test was modified.
