# Serving model: self-hosted product, managed-hosting later, not multi-tenant SaaS

Priority: P2 · Status: pending · Estimate: S

## Goal
Add an explicit "Serving Model" section to `VISION.md` that states, as a written
contract rather than an inferred one, how Canary is served:
1. **Self-hosted single-tenant binary is THE product** (and the competitive wedge).
2. **Optional managed hosting** of that *same single-tenant binary*, one isolated
   instance per customer, is a possible *later* offering (open-core / Plausible /
   PostHog model) — convenience/revenue, not a re-architecture.
3. **Multi-tenant SaaS is out by default** — a clustered store + tenant isolation
   would forfeit the single-binary elegance and put Canary on the incumbents' turf.

## Why now
Surfaced onboarding the first external reference consumer (Adminifi Habitat), whose
operator asked directly: "self-hosted, not SaaS — yeah?" The answer is yes, but today
it has to be reverse-engineered from `fly.toml` + scattered doc lines. `VISION.md`
states the *negative* ("Not multi-tenant SaaS by default", #79-80) and PRINCIPLES #7
says "No multi-tenant support (internal tool)", but the *positive* serving model is
implied, not declared. The architecture already commits to it — SQLite single-writer
on one machine **is** single-tenant — so the doc should say so plainly. The
competitive position (VISION: "does not try to out-feature the incumbents") depends on
the self-hosted wedge being intentional, not incidental.

## Scope / decision to record
- self-hosted single-tenant binary = the product and the wedge.
- managed hosting = run the SAME single-tenant binary per customer; a future
  revenue/convenience path, NOT multi-tenant SaaS. Don't build it now; don't foreclose
  it — PRINCIPLE #9 ("design for migration, don't build for it") already keeps the door
  open.
- multi-tenant SaaS = explicitly out by default; revisiting it is a deliberate,
  security-reviewed decision gated by #039 (external-user security/privacy foundation).

## Oracle
- [ ] `VISION.md` has a "Serving Model" section stating the three-way distinction above.
- [ ] It reconciles with "What Canary Is Not" (#79-80) and PRINCIPLES #2/#7 — cross-referenced, no contradiction.
- [ ] A new reader can answer "self-hosted, managed-later, or SaaS?" from `VISION.md` alone, without reading `fly.toml` or the code.

## Relationship to existing backlog
Pure doc/positioning; no code impact. Complements #039 (external-user security/privacy
foundation — the gate any hosted/multi-tenant productization must pass). Pairs with #055
(both are doc-truth fixes surfaced by the Habitat dogfooding pass).
