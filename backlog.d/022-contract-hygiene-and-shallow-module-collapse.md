# Contract hygiene and shallow-module collapse

Priority: high
Status: ready
Estimate: M

## Goal
Restore the `summary`-on-every-query-response invariant, collapse four shallow-module GenServers into a single ETS-table owner, and resolve the dashboard's product status — so Canary's agent-first contract is honest and the codebase sheds 300+ LOC of drift without changing behaviour.

## Non-Goals
- Add any new agent-facing endpoint (that is #023)
- Change the generic-webhook contract or payload shape
- Modify webhook HMAC signing, delivery ledger, or circuit-breaker *semantics* — only their module shape
- Remove the operator fallback entirely without first making an explicit keep-or-delete decision (see Notes — the dashboard sub-work is "decide, then execute," not "delete unconditionally")

## Oracle
- [ ] `curl -s -H "X-API-Key: $KEY" https://canary-obs.fly.dev/api/v1/query?window=1h | jq -e '.summary'` returns a non-null string (currently fails for the `errors_by_class` shape)
- [ ] `mix test test/canary/query/errors_test.exs` includes a test asserting every public query function returns a map with a non-empty `summary` key, and it passes
- [ ] `rg -l "use GenServer" lib/canary/alerter/ lib/canary/errors/` lists **two** files (`circuit_breaker.ex` and `cooldown.ex` merged into a new module; `dedup_cache.ex` and `rate_limiter.ex` also merged) — down from four, ideally to **one** `Canary.EtsTables` owner
- [ ] `mix test test/canary/alerter/ test/canary/errors/` passes with no semantic changes to `open?/1`, `should_probe?/1`, `record_failure/1`, `check/2`, `mark/1`
- [ ] `./bin/validate` green (fast + strict lanes); coverage holds at 81% core / 90% canary_sdk
- [ ] A decision on the operator dashboard is recorded in `docs/decisions/` (or equivalent) with one of two outcomes executed: **(a) delete** — routes removed from `router.ex`, `lib/canary_web/live/*` deleted, `CanaryWeb.DashboardAuth` removed, README updated to point operators at the query API / `mix release.remote_console`; or **(b) commit** — dashboard binding listed in `priv/openapi/openapi.json` (or a parallel contract doc), a `/dashboard/health` smoke test added, and a one-line entry in `VISION.md` under "What Canary Is" acknowledging a human fallback surface
- [ ] `git diff master --stat` shows net LOC **deleted**, not added (target: −300 LOC or better)
- [ ] No new footgun introduced (`CLAUDE.md` footgun list unchanged; specifically: Oban tables still via Ecto migration, `Canary.Repo` pool_size still 1, no new `rescue` scaffolding)

## Notes

**Why now.** The 2026-04-21 grooming investigation surfaced three concurrent drift patterns that together undermine the "highly focused, simple, elegant, agent-first" claim:

1. **Invariant drift — missing `summary` field.**
   `lib/canary/query/errors.ex:93-112` (`errors_by_class/1`) returns `{:ok, %{window: window, groups: groups}}`. Every peer in the same module (`errors_by_service`, `error_detail`, `error_groups`) *does* return `summary`. `PRINCIPLES.md` #1 "Agent-First" is load-bearing: *"Every query response includes a `summary` field."* A single violator silently breaks the contract agents rely on to skip follow-up queries.

2. **Shallow-module drift — four GenServers wrapping pure ETS.**
   - `lib/canary/alerter/circuit_breaker.ex` (66 LOC) — GenServer with only `init/1`. Literally a one-time named-table initializer dressed up as a process.
   - `lib/canary/alerter/cooldown.ex` (59 LOC) — `init/1` + `handle_info(:cleanup, _)`.
   - `lib/canary/errors/dedup_cache.ex` (66 LOC) — same shape.
   - `lib/canary/errors/rate_limiter.ex` (83 LOC) — same shape.
   Total: 274 LOC. Zero `handle_call`/`handle_cast`. All public functions operate directly on ETS from the caller process. These are table-lifecycle utilities, not stateful services. Ousterhout's shallow-module red flag (`PRINCIPLES.md` #5): *"The interface should be simple even when the implementation is complex."* Here, the interface is simple because the implementation is *also* simple — the GenServer wrapper adds zero hiding. It adds four supervision-tree nodes, four failure domains, four `GenServer.start_link` dances, and four sets of supervision tests.

3. **Surface drift — operator dashboard without a clear product role.**
   `lib/canary_web/router.ex:67-83` exposes four LiveView routes (`/dashboard/login`, `/dashboard/`, `/dashboard/errors`, `/dashboard/errors/:id`). Supporting code totals **673 LOC** across six files under `lib/canary_web/live/`. Zero OpenAPI binding. Zero agent consumer. `repo-brief.md` calls the dashboard a "fallback, not the product surface" — but 673 LOC is not a fallback, it's a second product. `PRINCIPLES.md` #7 "Code is a liability" says every line fights for its life; these lines haven't been forced to justify themselves since they landed.

**Responder-boundary check.** Pure refactor + contract restoration + operator-surface decision. Canary-internal. No webhook shape, no consumer-facing API change (except the `errors_by_class` response gaining a `summary` field, which is strictly additive for consumers who ignore extras).

**Execution sketch (one PR, three atomic commits).**

*Commit 1 — `fix(query): restore summary field on errors_by_class response`.*
Mirror the shape of `errors_by_service/1` — add `error_class: :aggregate`, total count, and a deterministic summary string. Add a test in `test/canary/query/errors_test.exs` that iterates over every public function in `Canary.Query.Errors` and asserts `summary` is a non-empty string. Make it a contract test so future additions can't regress. ~20 LOC added, ~0 deleted, but the test prevents the entire class of drift.

*Commit 2 — `refactor(core): collapse ETS table owners into Canary.EtsTables`.*
New module `lib/canary/ets_tables.ex`: one GenServer, `init/1` creates all four named tables, single `handle_info(:cleanup, _)` ticks through each table's TTL policy via a small strategy map. Public API modules (`Cooldown`, `CircuitBreaker`, `DedupCache`, `RateLimiter`) become plain modules — no `use GenServer` — exposing just their pure ETS-accessor functions. Supervision tree loses four children, gains one. Tests stay green by construction; update `application.ex` to start only `Canary.EtsTables`. Estimated net delete: ~180 LOC.

*Commit 3 — `chore(web): <delete-or-commit-to> operator dashboard`.*
Sub-decision (must be made before the commit): keep or delete. Default recommendation: **delete**. The operator use case (look at errors in a browser) is already served by `GET /api/v1/errors/{id}`, which any `curl | jq` session handles. If kept, bind the dashboard to a smoke test + an OpenAPI contract doc so it can't silently drift again. Either way, record the decision in a short ADR and link it in `VISION.md`.

**Risk list.**

- *Contract test for `summary` is hard to write generically* → worth it anyway; hand-roll if needed. Price of the invariant.
- *Supervision-tree collapse changes start-order dependencies* → walk `lib/canary/application.ex`; the four modules have no ordering dependencies on each other today, so the collapse is safe.
- *Dashboard delete surprises operators who used it* → mitigated by the ADR step and a grep for references in docs / runbooks before deletion.
- *Footgun: test-mode sandbox race reappears if boot-order shifts* → `Canary.Health.Manager.handle_info(:boot)` rescue pattern stays; no change needed to that module.

**Lane.** Lane 3 (structural) — ships independently, no cross-repo deps, unblocks nothing but makes the codebase honest. Ship first; it's the cleanest proof that "simple and elegant" is a live value.

Source: grooming session 2026-04-21. Parallel investigator evidence:
- Archaeologist (findings 1, 3, 5).
- Strategist (IncidentsResponse-summary flag overlaps here for #023, but `errors_by_class` is Canary's existing invariant violation; flagged by Archaeologist directly).
- Scout: Bugsink's lesson — "resist every gram of bloat" — applied to the shallow-module collapse and dashboard decision.
