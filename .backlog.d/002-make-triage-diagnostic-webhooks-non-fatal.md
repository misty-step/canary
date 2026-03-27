# Make Triage Diagnostic Webhooks Non-Fatal

Priority: high
Status: ready
Estimate: S

## Goal
Ensure Canary Triage acknowledges Canary diagnostic or test webhook events without crashing, raising `OTPError`, or creating bogus GitHub issues.

## Non-Goals
- Change the behavior of real `error.*` or lifecycle `health_check.*` events
- Introduce a new issue lifecycle model in triage
- Add a dashboard for webhook event inspection

## Oracle
- [ ] Given a webhook payload with `event: "test"` or another explicitly supported diagnostic event, when `CanaryTriage.Dispatch.handle/3` runs, then it returns a successful no-op instead of `{:error, {:unhandled_event, _}}`
- [ ] Given lifecycle health events and error events, when the triage dispatch tests run, then existing issue creation and recovery behavior remains unchanged
- [ ] Given the triage app test suite runs, when `cd triage && mix test test/canary_triage/dispatch_test.exs` is executed, then the new diagnostic-event behavior is covered deterministically

## Notes
This comes from GitHub #91. The current dispatch path already no-ops some non-lifecycle health events; this item extends that boundary so Canary-generated diagnostics do not page triage as failures.
