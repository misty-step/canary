# Credo custom check: `Canary.Checks.PreloadThenTake`

Priority: high
Status: ready
Estimate: S

## Goal

Ship a project-local Credo check that fails at lint time when a query
pipeline in `lib/canary/query/**` or `lib/canary/**/query*.ex` loads a
has-many relation via `preload(…)` and then truncates the result with
`Enum.take/2` (or `Enum.slice/2`, `Stream.take/2`). The pattern
advertises bounded output but executes unbounded reads — the exact
defect CodeRabbit flagged as "Bound signal fetch in incident detail
query" on PR #133.

## Non-Goals

- Ban `preload` in read models — it is the right tool when the
  truncation lives inside the preload subquery
  (`preload: [signals: ^from(s in IncidentSignal, limit: ^max)]`).
- Flag `Enum.take` outside read-model paths (e.g. rendering the top
  N of an already-bounded list is fine).
- Flag `Enum.take` on `Stream`s that wrap a lazy `Ecto.Query` stream
  with explicit `max_rows:` — Ecto stream + take is genuinely
  bounded.

## Oracle

- [ ] `mix credo --strict` fails on a synthetic file containing
      `Repo.preload(x, :signals) |> Map.update!(:signals, &Enum.take(&1, 25))`
      with a message pointing at the offending pipeline and the
      SQL-limit alternative
- [ ] `mix credo --strict` passes on the fixed shape from PR #133
      (`fetch_top_signals/3` with `limit: ^limit`)
- [ ] The check has no false positives on `lib/canary/` outside
      `lib/canary/query/**` and `lib/canary_web/` — verified by
      running `mix credo --strict` on the full tree post-merge
- [ ] Unit test at `test/credo_checks/preload_then_take_test.exs`
      covers: violating shape, bounded-preload shape (pass), outside
      read-model paths (no warning), `Stream.take` after a bounded
      `Repo.stream` (pass)
- [ ] `.credo.exs` lists the check under `checks.enabled`
- [ ] `./bin/validate --fast` green on the branch that introduces the check

## Notes

**Why now.** PR #133 landed the incident detail endpoint. The initial
read model used
`from(i in Incident, preload: [signals: …]) |> Repo.one() |> format`,
where `format` truncated signals to 25 in memory. CodeRabbit flagged
this as high-priority; the fix was two explicit queries (`count_signals`
+ `fetch_top_signals` with a SQL `LIMIT`). The generic pattern —
"advertise bounded, execute unbounded" — is directly replayable for
every future read model on a has-many relation. A Credo check shifts
it from "hope reviewers notice" to "cannot land past `--fast`."

**Detection shape (AST walk).**

Look for pipelines of the shape:

```
|> Repo.preload(…)  OR  |> preload([…], …)
  (one or more intermediate |>)
|> Enum.take(_, _)  OR  |> Enum.slice(_, _, _)  OR  |> Stream.take(_, _)
```

where at least one stage between the two operates on the preloaded
association (via `Map.update!/3`, `Map.get/2`, field access, or a
destructuring binding that later feeds `Enum.take`).

Implementation notes:

- Walk function bodies in files matching `~r"lib/canary/query/"` or
  `~r"/query\.ex$"` or files that `use Canary.DataCase` contexts
  flagged via `@moduledoc`. Start narrow; expand when false negatives
  appear.
- For each pipeline, collect the chain of `|>` nodes and check for
  the ordered pair `preload/…` → `take/slice` with no intervening
  `limit:` option on the preload.
- If the `preload/1` arg is a keyword list whose value is an
  `Ecto.Query.t()` expression containing `limit:`, the truncation is
  already in SQL — pass.

**Diagnostic message (template).**

> Bounded-payload antipattern at L#{line}: preload on `:#{field}`
> followed by `Enum.take/2` loads every row into memory before
> discarding most. Push the cap into SQL: either
> `preload: [#{field}: ^from(r in Rel, order_by: …, limit: ^max)]`,
> or split into `count_#{field}/1` + `fetch_top_#{field}/2`. See
> `lib/canary/query/incidents.ex:fetch_top_signals/3` for the
> reference shape.

**Execution sketch (one PR, two commits).**

*Commit 1 — `feat(lint): add Canary.Checks.PreloadThenTake custom Credo check`.*
Module at `lib/credo_checks/preload_then_take.ex`, enabled in
`.credo.exs`. Test at `test/credo_checks/preload_then_take_test.exs`
with happy/violating/edge-case snippets.

*Commit 2 — `docs(ops): link check to memory note and CLAUDE.md.*
Update `CLAUDE.md` footgun list with the pattern and name the
enforcement check. Cross-reference
`~/.claude/projects/-Users-phaedrus-Development-canary/memory/feedback_bounded_payloads.md`.

**Risk list.**

- *False positive on legitimate in-memory filters.* Scope the check
  to `lib/canary/query/**` and files using `Canary.DataCase` or
  `Canary.Repo` / `Canary.Repos.read_repo()` — this narrows the AST
  domain. Accept rare false positives with an explicit
  `# credo:disable-for-next-line Canary.Checks.PreloadThenTake`
  escape hatch for code where the in-memory filter is genuinely
  correct.
- *Stream + take on bounded query.* Explicitly skip when the pipeline
  contains `Repo.stream(_, max_rows: _)`; otherwise warn.

**Lane.** Lane 4 (hardening). Ships independently. Best built after
#026 (shares the Credo-check scaffolding).

**Share with spellbook?** The *concept* is generic (any ORM, any
paginated API). The *Credo implementation* is Elixir/Ecto-specific.
A companion spellbook reference (`bounded-payload-discipline.md`) lives
in the spellbook-layer ticket `#049`.

Source: `/reflect prevent-coderabbit-patterns` 2026-04-21, CodeRabbit
finding on PR #133 (comment id 3120644101), and
`feedback_bounded_payloads.md` memory note.
