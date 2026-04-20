# Product security controls

Priority: low
Status: done
Estimate: M

## Goal
Add scoped API keys and secret rotation so Canary can safely serve multiple
consumers with different trust levels.

## Non-Goals
- Full RBAC or multi-tenancy — scoped keys are sufficient for now
- OAuth/OIDC — API keys remain the auth mechanism
- Automated key rotation — document the manual process first

## Oracle
- [ ] Given an API key is created, when a scope is specified (e.g., `read-only`, `ingest-only`, `admin`), then the key is restricted to the specified operations
- [ ] Given a read-only key, when it attempts to POST /api/v1/errors, then the request is rejected with 403
- [ ] Given a key rotation is needed, when the operator follows the documented process, then old keys can be revoked and new keys issued without downtime
- [ ] Given scoped keys exist, when `mix test` runs, then scope enforcement is covered for each permission boundary

## Notes
Codex identified this during the 2026-04-01 audit. Currently all API keys have full
access to all endpoints. As consumer count grows beyond solo-operator use, different
services should have different access levels (e.g., ingest-only for error reporters,
read-only for dashboards, admin for target/webhook management).

## What Was Built

- Added first-class API key scopes: `admin`, `read-only`, and `ingest-only`
- Enforced scope checks at the router boundary so ingest, read, and admin paths fail with deterministic `403 insufficient_scope` responses
- Updated service onboarding to mint `ingest-only` keys and stopped emitting read/report snippets that reuse the ingest credential
- Extended key management and OpenAPI contracts to expose `scope` in requests and responses
- Documented manual zero-downtime key rotation in `docs/api-key-rotation.md`

## Verification

- `mix test test/canary/auth_test.exs test/canary/service_onboarding_test.exs test/canary_web/controllers/key_controller_test.exs test/canary_web/controllers/service_onboarding_controller_test.exs test/canary_web/controllers/service_onboarding_contract_test.exs test/canary_web/controllers/api_key_scope_test.exs test/canary_web/controllers/openapi_controller_test.exs`
- `./bin/validate --strict`
