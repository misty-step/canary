# Expand incident correlation failure-path coverage

Priority: high
Status: done
Estimate: S

## Goal
Make incident-correlation failure semantics explicit and deterministically tested across error ingest and health-check flows.

## Non-Goals
- Change the incident data model
- Add new correlation heuristics or LLM logic
- Re-architect the health checker

## Oracle
- [x] Given `Incidents.correlate/3` fails from the ingest path, when the relevant tests run, then the expected fail-open or fail-loud behavior is asserted
- [x] Given `Incidents.correlate/3` fails from the health-check path, when the relevant tests run, then the checker boundary behavior is deterministic and covered
- [x] Given unique-constraint fallback, rescue, and catch branches exist, when the incidents and ingest test suites run, then those branches are exercised
- [x] Given `mix test test/canary/incidents_test.exs test/canary/errors/ingest_test.exs test/canary/health/checker_test.exs` runs, then the failure-path coverage is green

## Notes
Correlation failures are currently easy to suppress and hard to reason about. For agent consumers, a silently-dropped correlation means the triage sprite never sees the incident.
Migrated from .backlog.d/003.

## What Was Built

- Extracted `Canary.CorrelationErrorTag` from duplicated `correlation_error_tag/1` in `ingest.ex` and `checker.ex`
- Added `Canary.IncidentCorrelation.safe_correlate/3` as the shared fail-open boundary for ingest and health-check callers, including invalid-return normalization for injected correlators.
- Added deterministic coverage for the health-check fail-open path plus the unique-constraint recovery branch inside `Incidents.correlate/3`.
- Added a `:self_report_errors` guard in `Canary.ErrorReporter` so failure-path tests can capture logs without recursive self-ingest.
- `{:ok, nil}` base-case test: `correlate/3` returns nil with no incident when target is "up"
- Rescue-clause error-shape test: proves `correlate/3` returns `{:error, {:exception, _}}` on internal exception (async sandbox isolation via raw spawn)
- Ingest fail-open test: error and group persist regardless of correlation outcome
- Unit tests for all 5 clauses of `CorrelationErrorTag.format/1`

### Workarounds
- Unique-constraint recovery is exercised through a test-only insert seam rather than a real SQLite writer race.
- Failure-path tests disable self-report ingestion with `Application.put_env(:canary, :self_report_errors, false)` so intentional `Logger.error` assertions do not recurse into Canary.
- No Mox: kept the dependency footprint flat by using an injected test correlator and a test-only `Incidents.correlate/4` seam.
