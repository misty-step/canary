# Canary Vision

Canary is the agent-first command center for production health: a small,
self-hosted union of error logging, uptime checks, operational events, and
incident coordination that fleets of agents can read from and write back to.

It is a small, self-hosted service that records errors, probes uptime, watches
check-ins, correlates incidents, keeps a durable timeline, and gives agents the
bounded context they need to respond without opening a human dashboard.

## The Problem

Production health tooling was built around humans staring at dashboards:
Sentry for application errors, UptimeRobot for uptime, PagerDuty or incident.io
for response, Datadog or Grafana for broad telemetry, and ad hoc glue between
them. That stack works for human teams with time to click, filter, and infer.
It is a poor default for autonomous agents, and an even worse default for fleets
of agents that need to divide incident triage without losing the story of what
happened.

Agents need a different surface:

- a compact answer to "what is happening now?"
- a durable event log they can replay after wake-up, crash, or handoff
- a conflict primitive so duplicate agents do not work the same incident
- scoped write-back for evidence, decisions, and ownership
- deterministic summaries that fit in transcripts and context windows
- setup and verification loops that prove a deployed app is actually covered

The missing product is not another dashboard. It is a coordination substrate
for agents watching production.

## The Bet

The next reliability loop is agent-operated:

1. Canary records a production signal: error, health transition, missed
   check-in, operational event, or incident correlation.
2. Canary sends a signed webhook as a wake-up hint, or an agent polls the
   report and timeline.
3. The agent claims the durable subject before acting.
4. The agent queries bounded context: report, incident detail, error group,
   target or monitor state, annotations, delivery diagnostics, and timeline.
5. The agent fixes, files, escalates, or deliberately dismisses outside Canary.
6. The agent writes back annotations, claim transitions, links, and evidence.
7. Another agent can replay the same timeline and see what happened.
8. The incident history remains useful after the urgency is gone: what signal
   fired, which agents responded, what they learned, what changed, and what
   still needs human attention.

Canary wins when that loop is boring, deterministic, and hard to misuse.

## What Canary Is

**A hyper-simple production health service.** One Rust service, one Docker
image, one SQLite database, one deployment target, and a small set of explicit
operator scripts. Complexity has to earn its way in.

**Agent-first observability.** API, CLI, SDK, and MCP-shaped tools are the
product surfaces. Humans use the same surfaces agents use. A browser dashboard
is not the product.

**A trigger surface for responder agents.** Canary should make it obvious how
an error, uptime failure, missed check-in, or operational event wakes one or more
triage agents while preserving enough context for them to avoid duplicate,
contradictory, or amnesiac work.

**An event and incident ledger.** The timeline is the durable source of truth.
Webhooks wake agents up; timeline replay tells them what is true.

**A coordination primitive.** Remediation claims let agents reserve a subject,
avoid duplicate work, transition ownership state, and release or verify the
work with evidence.

**A bounded context engine.** Reports, incident details, query rows, value
receipts, and summaries are deliberately compact. Canary should answer the
first question without forcing an agent through a raw data lake.

**A verification loop.** Integration is not complete because code was patched
or env vars exist. It is complete when deployed readback proves health,
ingest, query, webhook or monitor state, and a receipt.

## What Canary Is Not

- Not a general APM, tracing, logging, or metrics platform.
- Not an LLM trace/eval tool.
- Not an incident-command suite for human war rooms.
- Not a repo mutation, issue creation, or autonomous fix engine.
- Not a dashboard. Agents are the UI; operators inspect the same API and CLI.
- Not a semantic workflow engine hidden behind provider-specific agents.
- Not multi-tenant SaaS by default. External-user productization must be
  explicit, scoped, and security-reviewed.

Canary may expose MCP tools, SDKs, and CLIs, but those are adapters over the
same contract. They must not become parallel semantic APIs.

## Competitive Position

Canary does not try to out-feature the incumbents.

Use **Sentry** when the primary job is rich developer error monitoring,
source maps, release health, performance tracing, session replay, and mature
ecosystem integrations.

Use **UptimeRobot** when the primary job is cheap hosted uptime checks, status
pages, SSL/domain checks, and simple alerting without owning infrastructure.

Use **Better Stack** when the primary job is a hosted bundle of uptime, logs,
incident management, status pages, on-call, and AI-assisted operations.

Use **Datadog, New Relic, Grafana Cloud, Honeycomb, or OpenTelemetry stacks**
when the primary job is full-stack telemetry, distributed tracing, metrics,
high-cardinality exploration, dashboards, fleet analytics, or enterprise
observability governance.

Use **PagerDuty or incident.io** when the primary job is human on-call,
escalation policies, incident rooms, customer communications, and enterprise
incident process.

Use **Langfuse, LangSmith, or similar tools** when the primary job is LLM and
agent execution observability: prompts, traces, token cost, evaluations, and
human review of model behavior.

Use **Canary** when the primary job is smaller and sharper: give agents a
simple, self-hosted, production-health ledger they can read, replay, claim,
annotate, and respond to without a dashboard or a heavyweight observability
program.

## Current Product Truth

Canary is already past the "can it ingest and query?" stage. The active
contract includes:

- error ingest and grouped query readback
- HTTP targets and non-HTTP check-in monitors
- health, status, report, incident, error detail, and timeline read models
- signed webhooks plus a delivery ledger and diagnostics
- annotations on durable subjects
- remediation claims for agent ownership
- bounded operational events
- a Rust CLI with JSON envelopes for agent transcripts
- MCP manifest generation over the CLI surface
- TypeScript SDK instrumentation
- one-command integration discovery, patch planning, patching, and enrollment
- dogfood registry, audit, self-watch, and value receipt loops

The remaining gap is not raw capability. The gap is trust and usefulness:
alert reliability, stale evidence cleanup, responder safety, fewer
overclaimed integrations, and a cleaner agent handoff from signal to action.

## Product Principles

1. **Timeline before notification.** A webhook is only a wake-up hint. Durable
   replay is the correctness path.
2. **Claim before work.** Agents create a remediation claim before triage or
   repair so duplicate responders do not collide.
3. **Bound the first answer.** Every high-level read should fit in an agent
   transcript and carry a deterministic summary plus the next drill-down link.
4. **Write back evidence, not commands.** Annotations and claims record what
   happened. Downstream responders decide how to mutate repos, tickets, or
   infrastructure.
5. **No LLM on the request path.** Canary can feed agents, but Canary itself
   keeps summaries deterministic and auditable.
6. **Coverage requires readback.** A service is not covered until live Canary
   readback proves it.
7. **Self-watch must be external.** Canary watches apps; an independent witness
   watches Canary.
8. **Small beats complete.** Prefer a deep, simple module with a stable
   contract over broad observability surface area.

## Roadmap Bias

The near-term product direction is not "add every observability feature." It is:

- make alert-plane reliability and SLO/error-budget feedback trustworthy
- make responder context safe enough for arbitrary-agent consumers
- remove stale dogfood evidence and overclaiming in integration receipts
- make the integration loop prove deployed value faster
- keep MCP, CLI, SDK, and HTTP aligned to one semantic contract

The long-term goal is that an agent can ask one question and act:

> What is unhealthy, who is already working it, what context matters, and what
> is the next safe action?

Canary should be the simplest correct answer.
