# Canary Dogfood and Watchmen Design

Date: 2026-06-11

Status: historical Misty Step evidence snapshot. Current clean-room operators
should use `docs/networked-service-dogfooding.md` and
`priv/dogfood/owned_services.example.json`; do not treat the service names or
Vercel scopes below as product defaults.

## Goal

Every deployed owned application should be observable through Canary, and
agents should be able to answer "what is happening in Canary right now?"
without opening a human dashboard.

Canary remains agent-first. The right operator surface is a structured CLI,
JSON API, and eventually MCP server over the same read/admin API that agents
use. A browser dashboard stays intentionally out of scope unless a later ticket
finds a forcing function that the agent surface cannot satisfy.

## Historical Evidence

Commands run during the design pass:

- `vercel teams ls`
- `vercel project ls --scope misty-step --format=json`
- `vercel project ls --scope adminifi-growth --format=json`
- `vercel env ls production --cwd <linked repo> --format=json`
- `vercel env ls preview --cwd <linked repo> --format=json`
- `flyctl apps list`
- `bin/dogfood-audit --strict --window 24h`
- `curl -H "Authorization: Bearer $CANARY_API_KEY" "$CANARY_ENDPOINT/api/v1/report?window=24h&limit=20"`
- common health probes against `/healthz`, `/readyz`, `/api/health`,
  `/api/healthz`, and `/health`

Vercel scopes visible to the local CLI:

- `misty-step`
- `adminifi-growth`

Fly apps visible to the local CLI include `canary-obs`,
`linejam-canary-responder`, `memory-engine-api`, and `vox-cloud-api`; suspended
apps are still inventory state, but not active monitoring targets until
ratified.

Live Canary dogfood audit result on 2026-06-11:

- `bin/dogfood-audit --strict --window 24h` passed manifest structure.
- Canary reported `5 health surfaces monitored. 1 down (volume). 2994 errors across 2 services in the last 24 hours.`
- Active manifest targets were `chrondle`, `linejam`, `volume`, and `vulcan`.
- `canary-self` exists as an extra live target at `https://canary-obs.fly.dev/healthz`.
- `chrondle` had `2145` `TypeError` events in 24h.
- `sploot-web` had error groups in `/api/v1/report`, but `sploot` was not in the checked-in dogfood manifest or active target set.

Requested project coverage snapshot:

| Project | Deploy evidence | Canary evidence | Gap |
|---|---|---|---|
| `canary` | Fly `canary-obs`; `/healthz` and `/readyz` return 200 | `canary-self` target exists; `service=canary` had 0 errors in 24h | Self-watch only covers HTTP liveness, not worker continuity or an independent witness. |
| `vanity` | Vercel `vanity` -> `https://www.phaedrus.io` | Vercel env has Sentry vars, no Canary vars; local scan found no Canary code; no common health route returned 200 | Needs health route, ingest, target, and env enrollment. |
| `chrondle` | Vercel `chrondle` -> `https://www.chrondle.app` | Production and preview Vercel env include Canary browser vars; live target exists; `/api/health` returns 200 | Already enrolled for health and browser ingest, but current `TypeError` flood needs triage and regression proof. |
| `linejam` | Vercel `linejam`; Fly `linejam-canary-responder` | Production and preview Vercel env include server/browser Canary vars; live target exists; `/api/health` returns 200 | Strongest current integration; keep as reference implementation for webhook/responder coverage. |
| `misty-step` | Vercel `misty-step` -> `https://www.mistystep.io` | Vercel env has Sentry vars, no Canary vars; `/api/health` returns 200 | Needs dual-write or migration from Sentry-centric capture to Canary. |
| `trump-goggles-splash` | Vercel project -> `https://www.trumpgoggles.com` | local repo not linked to Vercel; no Canary hits; no common health route returned 200 | Needs local link/ownership proof, simple health route, target, and error capture. |
| `timeismoney-splash` | Vercel project -> `https://www.timeismoney.works` | local repo not linked to Vercel; no Canary hits; no common health route returned 200 | Same as `trump-goggles-splash`. |
| `sploot` | Vercel `sploot` -> `https://www.sploot.app` | Production Vercel env has Canary server vars; local repo has Canary reporter; `/api/health` returns 200; live errors appear as `sploot-web` | Needs preview env parity and target/manifest enrollment. |

## Coverage Contract

A deployed application is "using Canary" only when all relevant layers are
covered:

1. **Inventory**: the deployment is present in a registry with owner, platform,
   URL, repo path, environment, state, and last evidence timestamp.
2. **Health**: HTTP apps expose a health route and have a Canary target; non-HTTP
   apps use check-in monitors.
3. **Error ingest**: uncaught server errors, request errors, browser error
   boundaries where applicable, and explicit high-risk catches send sanitized
   events to Canary.
4. **Readback**: `GET /api/v1/query?service=...` and `GET /api/v1/report`
   show the service with bounded summaries.
5. **Agent affordance**: an agent can ask one command for status, recent errors,
   incidents, targets, monitors, and next actions without knowing raw routes.
6. **Verification**: a smoke command proves env presence, health route response,
   target enrollment, and one synthetic or real event readback.

## Watchmen Model

Canary watches applications. Canary itself needs two layers:

1. **Self-observation inside Canary**: `canary-self` targets `/healthz` and
   `/readyz`; Canary's logger path ingests `service=canary` errors directly;
   worker lifecycle health is exposed through #034.
2. **Independent witness outside Canary**: a tiny scheduled witness checks
   Canary from outside the Fly app, records a receipt outside Canary when Canary
   is unreachable, and sends a check-in to Canary when it is healthy. Good first
   substrates are GitHub Actions, a tiny Vercel cron, or a separate Fly app. The
   witness must not require Canary to be healthy to preserve failure evidence.

The user-facing shape should be:

```bash
canary summary --window 24h
canary services --state unhealthy --json
canary errors chrondle --window 24h
canary incidents --open
canary dogfood audit --strict
canary integrate plan --scope misty-step --project vanity
```

The same functions should become MCP tools once the CLI contract is stable.

## Integration Model

Manual SDK installation is necessary but not sufficient. Canary should provide
a one-command integration agent that can discover, patch, enroll, and verify a
project.

The agent flow:

1. `discover`: inspect repo framework, deployed platform, existing Sentry usage,
   health routes, env var names, and current Canary targets.
2. `plan`: produce a patch and enrollment plan without touching secrets.
3. `patch`: add SDK initialization, Next.js `onRequestError`, browser error
   boundary capture, a simple health route, and smoke tests when missing.
4. `enroll`: create scoped Canary keys, targets, monitors, and optional webhooks;
   use platform CLIs for env names, reading secret values only from stdin or a
   human-approved secret manager flow.
5. `verify`: run a deployed smoke that proves health target presence and query
   readback for the service.

For Vercel apps, the integration should also consider Log Drains as a lower-code
capture path where the account plan supports them. Vercel documents
[Drains](https://vercel.com/docs/drains) as a way to forward observability data
to custom HTTP endpoints, while
[`vercel env`](https://vercel.com/docs/cli/env) and `vercel env run` provide the
CLI path for environment verification.

## Non-Goals

- Reintroduce a browser dashboard.
- Move downstream repo mutation or remediation policy into Canary.
- Store Vercel, Fly, or GitHub secrets in the dogfood registry.
- Treat a project as covered just because env vars exist.
