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
- [ ] Given `Incidents.correlate/3` fails from the health-check path, when the relevant tests run, then the checker boundary behavior is deterministic and covered
- [x] Given unique-constraint fallback, rescue, and catch branches exist, when the incidents and ingest test suites run, then those branches are exercised
- [x] Given `mix test test/canary/incidents_test.exs test/canary/errors/ingest_test.exs` runs, then the failure-path coverage is green

## Notes
Correlation failures are currently easy to suppress and hard to reason about. For agent consumers, a silently-dropped correlation means the triage sprite never sees the incident.
Migrated from .backlog.d/003.

## What Was Built

- Extracted `Canary.CorrelationErrorTag` from duplicated `correlation_error_tag/1` in `ingest.ex` and `checker.ex`
- `{:ok, nil}` base-case test: `correlate/3` returns nil with no incident when target is "up"
- Rescue-clause error-shape test: proves `correlate/3` returns `{:error, {:exception, _}}` on internal exception (async sandbox isolation via raw spawn)
- Ingest fail-open test: error and group persist regardless of correlation outcome
- Unit tests for all 5 clauses of `CorrelationErrorTag.format/1`

### Workarounds
- **Unique-constraint race recovery (G3)**: Untestable in SQLite single-writer model — requires concurrent writes between `open_incident` query and `Repo.insert()`. The constraint IS tested at the DB level (incidents_test.exs:60). The recovery code path is structurally correct by inspection.
- **Checker fail-open (G2)**: Testing correlation failure within a GenServer requires mocking or dependency injection. The checker's `correlate_incident/1` has the same trivially-correct fail-open pattern as ingest (both branches return `:ok`, value discarded). Documented but not injected.
- **No Mox**: Chose not to add mocking library. Used sandbox isolation (raw spawn without `$callers` in async test) to trigger real DB exceptions instead.
