# Canary Agent-First Groom - 2026-06-19

## Source Matrix

| Lane | Status | Evidence | Contribution |
|---|---|---|---|
| Tidy mechanics | complete | `harness-kit-checks backlog ids-from-range origin/master..master` returned no ids; active files only `010`, `020`, `047`, `048`, `049` before edits | No archive move. Active queue needed shaping, not cleanup. |
| Vision | complete | `VISION.md`, `docs/agent-first-identity.md` | Vision is current and concrete: agent-first production health, timeline truth, claims before work, deterministic summaries, no dashboard. |
| Live state | complete | `bin/canary doctor --json`; `bin/canary dogfood audit --strict --json`; `bin/canary dogfood value --service linejam --json`; `bin/canary dogfood value --service chrondle --json` | Runtime is healthy with fresh witness receipts, but dogfood strict still fails on 45 coverage/evidence gaps. `linejam` is proven; `chrondle` is stale-registry-evidence despite clean live readback. |
| Product/value lane | complete | `VISION.md`, `docs/agent-first-identity.md`, `priv/dogfood/owned_services.json`, live dogfood commands | Canary's wedge is strong, but usefulness depends on stopping coverage/evidence overclaiming. Suggested promoting #049; lead kept it pending behind #048 safety. |
| Runtime/ops lane | complete | `bin/canary-witness`, worker modules, #047 | #047 is real: route readiness and alert-plane health are different. First slice must be induced impairment, not SLO math. |
| Security/privacy lane | complete | `server_auth.rs`, claims/annotations routes, OpenAPI, #048/#049 | #048 must own responder-write scope, minimized context, read-audit, public-ingest/relay, and CLI/MCP authority parity before broad responder/integration productization. |
| Architecture lane | complete | `wc -l`; `crates/canary-server/src/lib.rs`; `crates/canary-store/src/query.rs`; CLI integrate commands | Server/router and query modules are large, but immediate product risk is semantic trust: integration apply/readback and context authority. Consolidating #048/#049 was rejected to preserve safety-before-evidence sequencing. |
| Test/verification lane | complete | `dagger/src/index.ts`, `bin/canary-witness`, witness shell tests, #047/#049 | Gate is strong for the happy path. Missing falsifiers are induced alert-plane impairment, future check-in skew, and integration receipt overclaiming. |
| Agent readiness lane | complete | `bin/canary --help`, `docs/agent-inspection-cli.md`, MCP manifest, `.codex/agents` | CLI JSON and MCP manifest are good; missing a single cold-agent readiness proof. Added #050. |
| External exemplars | complete | Sentry, Better Stack, UptimeRobot, PagerDuty, incident.io, OpenTelemetry/Grafana/Honeycomb, Langfuse public positioning | Competitors are better for mature APM, incident command, generic uptime, broad telemetry, or LLM traces. Canary should borrow MCP/coding-agent integration and simple setup, but reject broad APM and LLM-on-request-path expansion. |

## Strategic Read

Canary is not missing a vision anymore. The current vision is sharp: a small
agent-first production health command center where timelines are truth,
webhooks wake agents, claims prevent duplicate work, and agents write back
evidence. The backlog should now make that trustable.

The live state says Canary is operationally healthy but not yet broadly
trustworthy as a consumption substrate. `doctor` reports healthy runtime and
fresh witness runs, while dogfood strict still reports 45 coverage/evidence
failures. That is the product gap: not "can Canary observe something?", but
"can agents trust Canary's signal, ownership, and receipts without rechecking
the world?"

## Decisions

1. Keep #047 as the best next pickup.
   The first slice is not burn-rate math. It is an induced alert-plane
   impairment proof showing that doctor and the external witness degrade when
   alertability is impaired even if `/readyz` remains route-ready.

2. Keep #048 before #049.
   Arbitrary responders need least-privilege claims/annotations, minimized
   context envelopes, read audit, webhook conformance, public-ingest/relay
   browser safety, and HTTP/CLI/MCP authority parity before integration apply
   can be trusted for third-party or arbitrary-agent consumers.

3. Keep #049 pending, but harden it.
   The ticket remains the integration evidence closure epic. It now explicitly
   forbids verified coverage from target/key creation alone, global webhook
   presence, stale evidence, or missing synthetic service-specific readback.

4. Add #050.
   Canary has good docs and machine surfaces, but no one-entrypoint cold-agent
   readiness proof. #050 packages the repo's agent operating proof into a
   script/skill/self-check plus redacted receipt.

5. Defer module-splitting backlog.
   `crates/canary-server/src/lib.rs` and `crates/canary-store/src/query.rs` are
   large enough to watch, but the current priority is semantic trust. A future
   refactor ticket is warranted when #047/#048/#049 implementation touches
   those modules enough that extraction pays for itself.

## Backlog Diff

- Updated `backlog.d/047-alert-plane-slo-burn-rate.md`:
  - retitled around proving alert-plane health separately from route readiness
  - added PRD summary, product requirements, technical design, and verification system
  - reordered children so induced impairment comes before SLO/burn-rate work

- Updated `backlog.d/048-responder-rich-context-safety-gate.md`:
  - retitled around external responder safety
  - added PRD summary, product requirements, technical design, and verification system
  - expanded scope to include public-ingest/relay browser safety and HTTP/CLI/MCP authority parity

- Updated `backlog.d/049-integration-evidence-closure.md`:
  - added PRD summary, product requirements, technical design, and verification system
  - hardened oracle around service-specific webhook coverage, synthetic readback, and stale-evidence failure
  - kept status `pending` behind #048 safety work

- Added `backlog.d/050-cold-agent-readiness-proof.md`:
  - P1/M pending ticket for one cold-agent proof path and redacted readiness receipt

- Updated `backlog.d/README.md`:
  - added #050 to active order
  - refreshed dependency map, lanes, active order, and migration notes

## Rejected Moves

- Do not consolidate #048 and #049 now. Safety and evidence are related but the
  sequence matters: responder authority and context safety must precede broad
  integration apply.
- Do not promote #049 to ready yet. The safety substrate from #048 should land
  first, or #049 risks implementing browser/MCP/apply mechanics around the
  wrong trust model.
- Do not add an AI/LLM feature epic. External competitors are adding AI helpers,
  but Canary's identity is deterministic summaries and no LLM on the request
  path. Agents can consume Canary; Canary should not become the agent brain.

## External Sources

- Sentry: <https://sentry.io/>
- Better Stack: <https://betterstack.com/> and <https://betterstack.com/uptime>
- UptimeRobot: <https://uptimerobot.com/>
- PagerDuty: <https://www.pagerduty.com/platform/incident-management/>
- incident.io: <https://incident.io/>
- OpenTelemetry: <https://opentelemetry.io/>
- Grafana: <https://grafana.com/products/cloud/features/>
- Honeycomb: <https://www.honeycomb.io/platform/opentelemetry>
- Langfuse: <https://langfuse.com/docs/observability/overview>

## Best Next Pickup

Pick up #047 child 1 and 3 together: implement the alert-plane verdict and the
induced impairment rehearsal. That slice directly tests the core product claim:
Canary can tell agents when its own alerting plane is not trustworthy, even if
the HTTP service is still up.
