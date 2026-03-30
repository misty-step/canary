# Make triage diagnostic webhooks non-fatal

Priority: high
Status: ready
Estimate: S

## Goal
Ensure Canary acknowledges diagnostic or test webhook events without crashing, raising `OTPError`, or creating bogus downstream actions.

## Non-Goals
- Change the behavior of real `error.*` or lifecycle `health_check.*` events
- Introduce a new issue lifecycle model
- Add a dashboard for webhook event inspection

## Oracle
- [ ] Given a webhook payload with `event: "test"` or another explicitly supported diagnostic event, when dispatch runs, then it returns a successful no-op instead of `{:error, {:unhandled_event, _}}`
- [ ] Given lifecycle health events and error events, when dispatch tests run, then existing behavior remains unchanged
- [ ] Given `mix test` runs, then the new diagnostic-event boundary is covered deterministically

## Notes
Prerequisite for sprite integration — a triage sprite subscribing to all events will receive test/diagnostic webhooks and must not crash on them.
Migrated from .backlog.d/002.
