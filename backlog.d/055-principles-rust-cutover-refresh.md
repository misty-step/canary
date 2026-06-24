# Refresh PRINCIPLES.md examples to match the Rust + SQLite implementation (post-cutover)

Priority: P3 · Status: pending · Estimate: S

## Goal
Update `PRINCIPLES.md` so its illustrative examples describe the current Rust + Axum +
SQLite service, not the retired Elixir/Phoenix one. The nine principles' *intent* still
holds; only the *examples* have drifted from reality and now mislead new contributors
and agents reading the principles as ground truth.

## Why now
The Rust cutover shipped (`docs/architecture/rust-cutover-evidence-2026-06-06.md`), and
`README.md` / `VISION.md` / Tech-Stack are already Rust+SQLite — but `PRINCIPLES.md` is
the lone stale surface, still citing Elixir/OTP constructs:
- **#5 Deep Modules** uses Elixir arity syntax: `Ingest.ingest/1`,
  `StateMachine.transition/4`, `Grouping.compute_group_hash/1`.
- **#8 Separation of Stateful and Stateless** references **GenServers + Oban**
  (Elixir/OTP), not the Rust worker/runtime model.
- **#9 Design for Migration** says "**Ecto** abstraction preserves the Postgres
  migration path" — Ecto is Elixir; the Rust store is `canary-store` over SQLite (WAL,
  one explicit writer boundary).

## Oracle
- [ ] `grep -niE "genserver|oban|ecto|\.(ingest|transition|compute_group_hash)/[0-9]" PRINCIPLES.md` returns nothing — or only inside an explicit "formerly, in the Elixir era" history note.
- [ ] #5 / #8 / #9 examples reference real Rust modules/types (e.g. the `canary-store` single-writer boundary, the Rust worker model) — each verifiable against `crates/`.
- [ ] The INTENT of all nine principles is unchanged; this is an examples refresh, not a principles rewrite.

## Relationship to existing backlog
Pure doc hygiene; follows the #032 live Rust write-path cutover. No runtime impact.
Pairs with #054 (both are doc-truth fixes surfaced by the Habitat dogfooding pass).
