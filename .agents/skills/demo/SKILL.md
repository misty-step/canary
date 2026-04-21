---
name: demo
description: |
  Generate demo artifacts: screenshots, GIF walkthroughs, video recordings,
  polished launch videos with narration and music. From raw evidence to
  shipped media. Also handles PR evidence upload via draft releases.
  Use when: "make a demo", "generate demo", "record walkthrough", "launch video",
  "PR evidence", "upload screenshots", "demo artifacts", "make a video",
  "demo this feature", "create a walkthrough", "scaffold demo",
  "generate demo skill".
  Trigger: /demo.
argument-hint: "[evidence-dir|feature|scaffold] [--format gif|video|launch] [upload]"
---

# /demo — Canary

Canary has no marketing surface and no human dashboard by design. Every demo
target is a functional product surface: the HTTP API (consumed by agents) and
the two SDKs. "Make a demo" in this repo means pick the audience first, then
capture the product surface they actually touch.

## Execution Stance

You are the executive orchestrator.
- Keep shot selection, evidence sufficiency, and final artifact approval on the lead model.
- Delegate planning, capture, and critique to separate focused subagents.
- Use a cold reviewer (fresh subagent, no capture context) for final quality judgment.
- Demo artifacts are evidence layered *on top of* `./bin/validate --strict`. They do not replace the gate.

## The Two Demo Modes

Canary is a multi-audience product. Pick the mode that matches the change:

| Mode | Audience | Surface | Primary artifact |
|------|----------|---------|------------------|
| 1. API / agent | AI agents, integration authors | HTTP API at `https://canary-obs.fly.dev` or `localhost:4000` | Shell script + asciinema/GIF of terminal flow |
| 2. SDK + integration | Downstream engineers | `canary_sdk/` (Elixir), `clients/typescript/` (TS), check-in monitors | Sample script + resulting `201` + `ERR-nanoid` receipt |

If the change touches more than one surface, capture all affected modes. A
change to the ingest pipeline needs Mode 1; a change to the SDKs needs Mode 2.
Do not conflate them.

## Mode 1: API / agent demo (primary)

Canary's north star is agent-native. The canonical demo is a reproducible
shell script an agent could replay itself as a conformance check.

### Required keys (before capture)

Demo scripts must use a **staging or test `ingest-only`** key plus a
**`read-only`** key. Never a bootstrap key. Never an admin key unless the demo
specifically covers an admin flow (target/monitor/webhook/key creation), and
even then scope the admin key to a demo-only instance.

```bash
export CANARY_BASE_URL="https://canary-obs.fly.dev"   # or http://localhost:4000
export CANARY_INGEST_KEY="ck_demo_ingest_..."         # scope: ingest-only
export CANARY_READ_KEY="ck_demo_read_..."             # scope: read-only
```

The bootstrap API key path (first-boot log grep of `"Bootstrap API key:"`) is
a one-time operator ritual. It is NOT a demo artifact — do not record it.

### Canonical narrative (the "replay guide" flow)

This mirrors `info.x-agent-guide` in `priv/openapi/openapi.json` — the
contract an agent would use to drive Canary end-to-end. Use these exact
endpoints, in this order, against a seeded service (e.g. `service=demo-api`):

1. **Ingest an error.** `POST /api/v1/errors` with `Authorization: Bearer
   $CANARY_INGEST_KEY`. Show `201 Created` with `{"id": "ERR-<nanoid>",
   "group_hash": "...", "is_new_class": true}`. A second identical POST must
   show `is_new_class: false` — proves deterministic grouping.
2. **Query recent errors.** `GET /api/v1/query?service=demo-api&window=1h`
   with `$CANARY_READ_KEY`. Show the bounded payload and the `summary` field.
   The `summary` is the product's differentiating claim — if it is not
   visible in the captured output, the shot is invalid.
3. **Unified report.** `GET /api/v1/report?window=1h` with `$CANARY_READ_KEY`.
   Show the single payload folding health + error groups + incidents +
   recent transitions, each with NL summaries and no LLM in the loop.
4. **Timeline.** `GET /api/v1/timeline?service=demo-api&window=24h&limit=50`.
   Call out that these payloads are identical in shape to what outbound
   webhook deliveries carry — same canonical event.
5. **Subscribe and test a webhook.** `POST /api/v1/webhooks` (admin key if
   demoing subscription; otherwise use a pre-seeded subscription).
   `POST /api/v1/webhooks/:id/test` fires a non-business `canary.ping`
   without writing to the timeline. Capture the HMAC signature header and
   the `X-Delivery-Id` — highlight at-least-once + dedupe semantics.
6. **OpenAPI contract.** `GET /api/v1/openapi.json` (public, no auth).
   Pipe through `jq '.info."x-agent-guide"'` and show the replay guide that
   agents consume. This closes the loop: the demo IS the guide.

### Artifact layout

```
tmp/demo/<YYYY-MM-DD>-<slug>/
  api-walkthrough.sh          # reproducible script, executable, redacted
  api-walkthrough.cast        # asciinema recording
  api-walkthrough.gif         # ffmpeg GIF for PR/README embedding
  responses/
    01-ingest.json            # redacted curl output, each step
    02-query.json
    03-report.json
    04-timeline.json
    05-webhook-test.headers
    06-openapi-agent-guide.json
```

### Redaction (mandatory before upload)

Every captured artifact must be scrubbed:

- `sed -E 's/Bearer [A-Za-z0-9_\-]+/Bearer REDACTED/g'` over every transcript.
- Strip webhook HMAC secrets from `POST /api/v1/webhooks` responses.
- Replace real tenant service names with `demo-api` before recording.
- Inspect redacted output manually before release upload.

### Rate-limit hygiene

Ingest is behind `:ingest_rate_limit` and queries are behind
`:query_rate_limit` (see `lib/canary_web/router.ex`). Demos loop N=3–5 times
at most. Do not flood a live instance — if you need volume, run against a
local `mix phx.server` with a freshly-migrated DB.

### Agent-replayability check (required)

The captured `api-walkthrough.sh` must be idempotent-ish and self-describing.
An agent reading `openapi.json#/info/x-agent-guide` plus this script should
reach the same final state. Verify by running it twice in the same window —
first run shows `is_new_class: true`, second shows `is_new_class: false` on
the repeated ingest.

## Mode 2: SDK + integration demo

Canary ships two first-party SDKs and a check-in monitor surface for
non-HTTP runtimes. A change to any of these requires a Mode 2 capture.

### Elixir SDK — `canary_sdk/`

The `Canary.SDK.Handler` module (see `canary_sdk/lib/canary_sdk/handler.ex`)
is a `:logger` handler that ships errors to Canary from any Elixir app.
Demo by attaching the handler in a sample IEx session or a throwaway Mix
project:

```elixir
:logger.add_handler(:canary, Canary.SDK.Handler, %{
  config: %{
    endpoint: System.get_env("CANARY_BASE_URL"),
    api_key: System.get_env("CANARY_INGEST_KEY"),
    service: "demo-elixir"
  }
})

Logger.error("demo error from elixir sdk", crash_reason: {RuntimeError, []})
```

Capture: terminal transcript showing the handler attaching, the
`Logger.error/2` call, and the resulting `201` returned by Canary (print
the response). Cross-reference to `GET /api/v1/query?service=demo-elixir`
to show the error landed with a deterministic `group_hash`.

Coverage threshold for `canary_sdk/` is **90%** — a SDK demo that lowers
coverage is a broken demo. Run `./bin/validate --strict` before the
capture to confirm the package gates are green.

### TypeScript SDK — `clients/typescript/`

The TS SDK is built with `tsup` and tested with `vitest`. Demo via a
throwaway Node script:

```ts
import { initCanary, captureException } from "@canary/sdk";

initCanary({
  endpoint: process.env.CANARY_BASE_URL!,
  apiKey: process.env.CANARY_INGEST_KEY!,
  service: "demo-ts",
  scrubPii: true,
});

try {
  throw new Error("demo error from ts sdk");
} catch (err) {
  const resp = await captureException(err, { severity: "error" });
  console.log(resp); // { id: "ERR-...", group_hash: "...", is_new_class: true }
}
```

Capture: the Node run transcript + a follow-up
`GET /api/v1/errors/ERR-<nanoid>` showing the ingested group. Core
message: one SDK call, deterministic grouping, `summary` field on lookup.

### Check-in monitors (non-HTTP runtimes)

For desktop apps, cron jobs, and workers, Canary uses check-in monitors
instead of HTTP targets. See `docs/non-http-health-semantics.md`.

Demo sequence:

1. Create the monitor — `POST /api/v1/monitors` with `mode: "schedule"` or
   `"ttl"`, `expected_every_ms`, `grace_ms`.
2. Heartbeat — `POST /api/v1/check-ins` with `monitor_id` and
   `status: "alive"`.
3. Let the grace window elapse without a check-in, then `GET /api/v1/health-status`
   to show the monitor transitioned to degraded/down without generating
   an error group.

Capture: script + transcript + `summary` output at each step.

## PR-evidence variant

When `/demo` is invoked on a PR, produce upload-ready evidence for the PR
description:

- **API response before/after.** For any change that modifies a response
  shape, capture redacted JSON snapshots of the affected endpoint against
  `master` and the PR branch. Diff them.
- **Webhook payload diff.** If the change touches
  `lib/canary/webhooks/delivery.ex` or any event payload builder, capture
  the before/after payload of the relevant event via
  `POST /api/v1/webhooks/:id/test` and the live event.
- **SDK transcript.** If the change touches `canary_sdk/` or
  `clients/typescript/`, include the integration run transcript with the
  resulting `201` payload.

Upload via:

```bash
gh release create qa-evidence-pr-${PR_NUMBER} --draft --title "PR ${PR_NUMBER} evidence" \
  tmp/demo/<slug>/*
gh pr comment ${PR_NUMBER} --body "Demo artifacts: <release URL>"
```

See the upstream `references/pr-evidence-upload.md` for the full protocol.

## Canary-specific guardrails (do not skip)

- **Scoped keys only.** `$CANARY_INGEST_KEY` is `ingest-only`; `$CANARY_READ_KEY`
  is `read-only`. Admin-scope demos go against a demo-only instance.
- **Redact every `Authorization: Bearer …` before upload.** Non-negotiable.
- **Never demo the bootstrap key path.** That is a first-boot log ritual.
- **Respect rate limits.** `:ingest_rate_limit` and `:query_rate_limit` apply;
  loop ≤5 times per capture. Heavy volume → local `mix phx.server`.
- **Every captured API response must show `summary`.** The deterministic
  template summary is the product claim. Missing `summary` = invalid shot.
- **No LLM on the request path, ever.** Summaries are templates
  (invariant in `CLAUDE.md`). A demo script that routes Canary responses
  through an LLM to "explain" them violates the product thesis — don't.
- **Coverage thresholds are load-bearing.** Core **81%**, `canary_sdk` **90%**.
  Demo prep does not lower them.

## Workflow: Plan → Capture → Critique → Upload

Each phase is a separate subagent. The critic inspects artifacts cold
(no capture context) to prevent self-grading.

1. **Plan.** Identify which of Modes 1/2 apply. Build a shot list tied to
   specific endpoint paths or SDK call sites. Pick target environment
   (live `canary-obs.fly.dev` vs local `mix phx.server`).
2. **Capture.** Execute the plan. Every "after" has a paired "before."
   Redact on the way out, not as a post-processing step.
3. **Critique.** Fresh subagent validates: correct mode for the change,
   before/after pairing, `summary` field visible, keys redacted,
   endpoints cited by full path, no bootstrap key leakage, no marketing
   framing.
4. **Upload.** `gh release create qa-evidence-pr-${N} --draft` + PR
   comment with the release URL.

## FFmpeg quick reference

```bash
# WebM -> GIF (800px, 8fps, 128 colors) — terminal walkthroughs
ffmpeg -y -i input.webm \
  -vf "fps=8,scale=800:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=bayer" \
  -loop 0 output.gif

# asciinema -> GIF
agg input.cast output.gif --speed 1.5 --theme monokai
```

## Relationship to the gate

Demo artifacts do NOT gate merge. Merge is gated by `./bin/validate --strict`
(the canonical strict Dagger entrypoint — see the repo brief). Demos are
evidence layered on top: they prove the change is demoable, not that the
code is correct. Run `./bin/validate --strict` first, capture demos second.

## References

- Canonical endpoint examples: `README.md`
- Agent contract: `priv/openapi/openapi.json` (and its `info.x-agent-guide`)
- Endpoint surface: `lib/canary_web/router.ex`
- Non-HTTP health model: `docs/non-http-health-semantics.md`
- Dogfood audit: `docs/networked-service-dogfooding.md`, `bin/dogfood-audit`
- Upstream reference skill (capture mechanics, Remotion, TTS):
  `/Users/phaedrus/Development/spellbook/skills/demo/`

## Gotchas

- **Default-state evidence proves nothing.** A `GET /report` with an empty
  error list is not a demo — it is a snapshot of the empty state. Seed the
  state deliberately.
- **Self-grading is worthless.** The critic subagent inspects artifacts
  cold.
- **The `summary` field is the whole point.** If the captured payload hides
  it (wrong `jq` filter, truncated output), the shot is invalid — recapture.
- **Do not narrate against Sentry/Uptime Robot.** Canary replaces their
  core function for agents, but the demo is about what Canary does for the
  operator/agent in front of it — not a competitive teardown.
