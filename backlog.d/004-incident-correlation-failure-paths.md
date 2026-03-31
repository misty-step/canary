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
- [x] Given `mix test test/canary/incidents_test.exs test/canary/errors/ingest_test.exs` runs, then the failure-path coverage is green

## Notes
Correlation failures are currently easy to suppress and hard to reason about. For agent consumers, a silently-dropped correlation means the triage sprite never sees the incident.
Migrated from .backlog.d/003.

## What Was Built

- Extracted the injectable correlation engine to `Canary.Incidents.Correlation`, keeping `Canary.Incidents` as the stable public interface while preserving deterministic tests for unique-constraint fallback plus raise/throw handling.
- Added `Canary.IncidentCorrelation.safe_correlate/3` as the single boundary helper for ingest and health-check callers so both paths convert correlator exceptions into explicit `{:error, ...}` tuples before logging and continuing.
- Added focused coverage for the ingest and health-check fail-open boundaries, including raised and thrown correlator failures, plus the conflict-fallback path inside incident correlation.
- Added a `:self_report_errors` guard in `Canary.ErrorReporter` so tests can disable self-report recursion without removing logger handlers.

### Workarounds

- Failure-path tests disable self-report ingestion with `Application.put_env(:canary, :self_report_errors, false)` so intentional `Logger.error` assertions do not recursively ingest Canary's own test failures.
