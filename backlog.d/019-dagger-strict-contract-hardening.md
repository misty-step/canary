# Dagger strict contract hardening

Priority: medium
Status: done
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

## What Was Built

- Replaced the `Ci.strict` source-text scrape in `dagger/scripts/ci_contract_validation.py` with a structural parser that walks the `Ci` class, discovers `@check()` methods from class shape, and extracts top-level `await this.<gate>(repo)` calls without depending on exact formatting.
- Added parser fixtures that cover semicolonless `strict` bodies, multiline call formatting, decorator spacing, and newly-added `@check()` gates so harmless TypeScript refactors stay green.
- Upgraded strict contract failures to report the expected and actual gate order plus missing or extra gates, so drift is diagnosable from one error message.

## Verification

- `python3 dagger/scripts/ci_contract_validation.py`
- `./bin/validate --strict`
