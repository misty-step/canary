# Operator Dashboard Removal

Decision date: 2026-04-21

## Problem

Canary's raison d'être is agent-first observability. `VISION.md` is explicit:

> **Not a dashboard (agents are the UI).**

Yet `lib/canary_web/live/` contained 673 LOC of LiveView surface
(`DashboardLive`, `ErrorsLive`, `ErrorDetailLive`, `LoginLive`,
`DashboardComponents`, `DashboardAuth`) plus a login controller, a
bcrypt-gated password flow, a dashboard layout, an asset bundle, and a
matching test tree. Zero OpenAPI binding, zero agent consumer, zero
contract tests. `repo-brief.md` called it "a fallback, not the product
surface" — but 673 LOC is not a fallback, it is a second product
drifting without a forcing function.

The 2026-04-21 grooming investigation surfaced this as one of three
contract-hygiene drift patterns. This ADR records the decision made
under backlog item #022 ("Contract hygiene and shallow-module collapse").

## Options considered

### (a) Delete the dashboard — **selected**

Remove routes, LiveViews, login controller, dashboard layout, dashboard
auth hook, dashboard tests, and the `DASHBOARD_PASSWORD` config surface.
Operators consume the same data agents do: the query API (`GET
/api/v1/query`, `GET /api/v1/errors/:id`, `GET /api/v1/status`, etc.) or
the remote console (`flyctl ssh console --app canary-obs -C "bin/canary
remote"`).

Strengths:

- Honors `VISION.md`: "Not a dashboard (agents are the UI)."
- Drops ~900 LOC (673 LiveView + ~200 tests + config + layout +
  `DashboardAuth` + login plumbing). No consumer complains — no external
  consumer exists.
- Removes `DASHBOARD_PASSWORD` as a runtime concern and an operational
  failure mode (the "publicly accessible if unset" warning).
- Leaves `priv/static/assets/phoenix_live_view.min.js` and the font
  bundle unused — those are deleted alongside.

Weaknesses:

- Operators who manually inspected `/dashboard` must switch to `curl |
  jq` against the API or `bin/canary remote`. The README update tells
  them how. This is a tiny blast radius: Canary has one operator today.

### (b) Commit to the dashboard

Bind it to an OpenAPI contract doc (or a parallel schema), add a
`/dashboard/health` smoke test, add a `VISION.md` entry under "What
Canary Is" acknowledging a human fallback surface. Justify 673 LOC with
a use case.

Strengths:

- Preserves a human-friendly observability path for operators who want
  it.

Weaknesses:

- Contradicts `VISION.md`'s "Not a dashboard" claim.
- Adds a second product surface with no agent consumer, no differentiator
  over `curl | jq` against `GET /api/v1/status`, and no forcing function
  against drift.
- The "fallback" framing is already failing: no smoke test, no contract,
  no agent consumer has noticed it exists.

## Decision

Select (a). The dashboard is deleted. Operators read through the query
API or `bin/canary remote` — the same primitives agents use.

## Execution

Landed in `refactor(web): remove operator dashboard` as part of backlog
item #022 on branch `deliver/022-contract-hygiene`. Changes:

- Remove `/dashboard/*` and `/dashboard/login*` routes from `lib/canary_web/router.ex`.
- Delete `lib/canary_web/live/` (all six files).
- Delete `lib/canary_web/controllers/login_controller.ex`.
- Delete `lib/canary_web/layouts/dashboard.html.heex` and
  `lib/canary_web/layouts/root.html.heex` and `lib/canary_web/layouts.ex`.
- Drop `DashboardComponents` import from `lib/canary_web.ex`; remove the
  LiveView helpers that only existed to assemble dashboard LiveViews.
- Remove the LiveView socket mount and static asset plumbing from
  `lib/canary_web/endpoint.ex`.
- Delete `priv/static/assets/phoenix_live_view.min.js`,
  `priv/static/assets/phoenix.min.js`, and `priv/static/fonts/*`.
- Remove `dashboard_password_hash` / `dashboard_auth_version` from
  `config/runtime.exs`.
- Delete `test/canary_web/live/`.
- Drop the stray `live "/dashboard"` assertion from
  `test/canary_web/controllers/service_onboarding_controller_test.exs`.
- Update `README.md` and the demo/qa skills to point operators at the
  query API and `flyctl ssh console` instead of `/dashboard`.
- Drop `phoenix_live_view`, `phoenix_html`, and `lazy_html` from
  `mix.exs` — no remaining users. `bcrypt_elixir` stays; `Canary.Auth`
  still hashes API keys.

## Follow-ups

None. The removal is self-contained; no downstream responders depend on
the dashboard surface.
