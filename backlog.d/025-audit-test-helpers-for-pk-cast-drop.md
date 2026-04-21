# Audit test helpers for Ecto PK cast-drop

Priority: low
Status: ready
Estimate: S

## Goal
Find and fix every test helper that passes a custom string primary key (`ERR-`, `INC-`, `WHK-`, `TGT-`, `MON-`, or a hash) through `Module.changeset(attrs)` instead of setting it on the struct, so the CLAUDE.md footgun #1 can't silently re-land.

## Non-Goals
- Re-derive the rule — it's in `CLAUDE.md` and reiterated in every schema whose PK isn't in `@required`/`@optional`.
- Add a Credo check for this pattern (deferred; worth its own ticket once the audit surfaces the scope).

## Oracle
- [ ] `rg -n "\\|> .+\\.changeset\\(Map\\.merge" test/` returns only helpers that either (a) don't have a custom PK, or (b) use the `%Schema{pk: id} |> Schema.changeset(attrs_without_pk)` pattern
- [ ] `mix test` green after each fix
- [ ] A one-line comment or a shared helper documents the correct pattern near any remaining `Map.merge(defaults, attrs)` idiom

## Notes

**Why now.** #022 introduced `test/canary/query/errors_test.exs` with a helper that copied the `%Struct{} |> Struct.changeset(Map.merge(defaults_with_pk, attrs))` pattern from `test/canary/query_test.exs:23-25`. CodeRabbit caught the copy (PR #132, comment id 3120469488) via the coding guideline "Custom string primary keys ... must be set on the struct before passing to changeset." The helper was fixed in-branch for the new file; the pre-existing copy in `test/canary/query_test.exs` stayed as-is because it was out of scope for #022.

**Open question — does it actually regress anything?** `test/canary/query_test.exs` has 25 tests that currently pass. Either SQLite accepts NULL PKs (unlikely; `@primary_key {:group_hash, :string, autogenerate: false}` + `NOT NULL` constraint via ecto_sqlite3), or the `ErrorGroup` PK ends up set somehow, or tests happen to not exercise the PK in a way that would blow up. The audit should start by adding a targeted assertion (`assert group.group_hash == "known-hash"`) to one of the existing tests to catch whether the silent-drop is actually happening.

**Responder-boundary check.** Pure test-quality cleanup. No product surface.

**Scope.** One PR. Grep, audit, fix, repeat. Likely ≤20 LOC of changes.

**Lane.** Lane 4 (hardening) — independent, small.

Source: CodeRabbit review on PR #132 during #022 delivery.
