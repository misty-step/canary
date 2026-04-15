# Dagger strict contract hardening

Priority: medium
Status: ready
Estimate: S

## Goal
Harden the `Ci.strict` contract check so it stays reliable across harmless
source refactors while still proving that local strict validation matches the
intended Dagger gate set.

## Non-Goals
- Changing the current strict gate set
- Reducing or bypassing any CI quality gate
- Replacing the existing deterministic ci-contract suite

## Oracle
- [ ] Given `ci_contract_validation.py` verifies `Ci.strict`, when the Dagger TypeScript source is reformatted or lightly refactored, then the contract check still passes without depending on fragile token shapes
- [ ] Given new `@check()` gates can be added over time, when that happens, then contract validation still proves `Ci.strict` includes them without requiring a brittle source-text parser
- [ ] Given the strict contract becomes invalid, when validation fails, then the failure message points to the missing or extra gate clearly enough to fix without manual spelunking

## Notes
Review on 2026-04-14 cleared the current implementation for ship, but flagged
the new source parser as intentionally narrow: it scans raw text, assumes
specific `await this.<name>(repo)` call shapes, and does not understand
TypeScript syntax. A second pass should replace that with a more structural
assertion.
