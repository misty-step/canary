# Connect-a-service workflow

Priority: medium
Status: done
Estimate: M

## Goal
Let a solo operator connect a new service to Canary through one opinionated path instead of stitching together API keys, target setup, snippets, and report queries by hand.

## Non-Goals
- Replace the existing REST API surface
- Build a generic multi-tenant onboarding system
- Ship runtime-specific SDKs for every language in this item

## Oracle
- [x] Given a fresh Canary instance, when an operator follows the new flow, then they can create an API key, register a target, and obtain exact error-reporting snippets from one place
- [x] Given the flow is completed for a sample service, when the operator opens `/api/v1/report` and the dashboard, then the new service appears without extra setup steps
- [x] Given the onboarding surface exists, when `mix test` runs, then coverage proves the key creation, target registration, and onboarding composition behavior stays green

## Notes
Canary already has the core primitives, but the user journey is an assembly exercise spread across API routes and README examples. With annotations (001), onboarding should also document annotation conventions for agent consumers.
Migrated from .backlog.d/001.

## What Was Built

- Added `POST /api/v1/service-onboarding`, an authenticated composition endpoint that creates a target, generates a fresh ingest key, and returns exact snippets plus report/dashboard links in one response.
- Introduced `Canary.ServiceOnboarding` as the opinionated workflow boundary for request validation, SSRF checks, duplicate-target rejection, rollback on key-generation failure, and deterministic snippet generation.
- Documented the flow in both `README.md` and `priv/openapi/openapi.json` so operators and agents can discover the onboarding contract without stitching together separate keys/targets docs.
- Added focused coverage for success, validation failure, duplicate-service rejection, rollback behavior, and the end-to-end visibility check through both `/api/v1/report` and `/dashboard`.

## Verification

- `mix test test/canary/service_onboarding_test.exs test/canary_web/controllers/service_onboarding_controller_test.exs test/canary_web/controllers/openapi_controller_test.exs test/canary_web/controllers/report_controller_test.exs test/canary_web/live/dashboard_live_test.exs`
- `mix test`
- `mix format --check-formatted`
- `./bin/dagger check`
