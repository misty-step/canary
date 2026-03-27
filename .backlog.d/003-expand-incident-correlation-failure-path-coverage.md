# Expand Incident Correlation Failure-Path Coverage

Priority: high
Status: ready
Estimate: S

## Goal
Make incident-correlation failure semantics explicit and deterministically tested across error ingest and health-check flows.

## Non-Goals
- Change the incident data model
- Add new correlation heuristics or LLM logic
- Re-architect the health checker in this item

## Oracle
- [ ] Given `Incidents.correlate/3` fails from the ingest path, when the relevant tests run, then the expected fail-open or fail-loud behavior is asserted instead of relying on log inspection
- [ ] Given `Incidents.correlate/3` fails from the health-check path, when the relevant tests run, then the checker boundary behavior is deterministic and covered
- [ ] Given unique-constraint fallback, rescue, and catch branches exist, when the incidents and ingest test suites run, then those branches are exercised without race-prone timing tricks
- [ ] Given the work is complete, when `mix test test/canary/incidents_test.exs test/canary/errors/ingest_test.exs` runs, then the failure-path coverage is green

## Notes
This consolidates GitHub #81 with the archaeologist finding that correlation failures are currently easy to suppress and hard to reason about.
