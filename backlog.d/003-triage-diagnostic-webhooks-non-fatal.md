# Make triage diagnostic webhooks non-fatal

Priority: high
Status: done
Estimate: S

## Goal
Ensure Canary acknowledges diagnostic or test webhook events without crashing, raising `OTPError`, or creating bogus downstream actions.

## Non-Goals
- Change the behavior of real `error.*` or lifecycle `health_check.*` events
- Introduce a new issue lifecycle model
- Add a dashboard for webhook event inspection

## Oracle
- [x] Given a webhook payload with `event: "test"` or another explicitly supported diagnostic event, when dispatch runs, then it returns a successful no-op instead of `{:error, {:unhandled_event, _}}`
- [x] Given lifecycle health events and error events, when dispatch tests run, then existing behavior remains unchanged
- [x] Given `mix test` runs, then the new diagnostic-event boundary is covered deterministically

## Notes
Prerequisite for sprite integration — a triage sprite subscribing to all events will receive test/diagnostic webhooks and must not crash on them.
Migrated from .backlog.d/002.

## What Was Built

- Split `EventTypes` into timeline events (9 business events) and diagnostic events (`canary.ping`), with a two-function interface: `valid?/1` (webhook subscription gate) and `timeline/0` (persistence/query gate)
- `ServiceEvent` changeset validates against `timeline()` — diagnostic events cannot be persisted
- Timeline API rejects `canary.ping` as a filter with 422, listing only timeline-valid events in the error message
- Webhook creation accepts `canary.ping` in event subscriptions (201)
- 6 new unit tests for EventTypes, 1 new integration test each for webhook creation and timeline rejection
- 298 tests total, 0 failures

### Workarounds
- None. The `WebhookController.test/2` action already handled `canary.ping` delivery correctly — this change makes the event type system aware of the distinction rather than relying on ad-hoc bypass logic.
