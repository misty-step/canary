# Pin the live incident-payload shape before anyone "fixes" it

Priority: P0 · Status: ready · Estimate: S

## Goal
The exact JSON shape canary's incident emitter produces today is pinned by a committed fixture and a conformance test, so any change to it fails CI instead of silently breaking downstream triage.

## Oracle
- [ ] A committed fixture captures the CURRENT live emitter output (top-level key `incident`, no `schema_version`) generated from `incident_payload` in `crates/canary-store/src/incidents.rs:505`.
- [ ] A test asserts the emitter output matches the fixture byte-for-byte (or field-for-field); mutating the payload shape fails the test.
- [ ] `docs/architecture/canary-bitterblossom-triage-contract-2026-07-01.md` is corrected to describe the shape that actually ships (`incident`, no schema_version) with a note that the `subject`+`schema_version` form is a FUTURE coordinated migration, not the current contract.

## Notes
DO NOT change the emitter to match the contract doc — that direction silently breaks bitterblossom triage (its task.toml JSON pointer filters on `/incident/service`; a renamed key returns HTTP 200 `{"filtered":...}` and no triage run ever fires). Tonight's move is pin-reality-and-fix-the-doc; the coordinated rename to `subject`+`schema_version:1` is daylight work requiring a lockstep bitterblossom change.
**Why:** 2026-07-01 composition seam audit, Seam 2 — ranked the #1 most-likely-to-break seam; three divergent shapes exist (live emitter, BB's ingress test at `tests/ingress.rs:134`, the contract doc) and only the emitter's is real.
