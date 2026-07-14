# Canary Vision

Canary is the agent-first command center for production health: a small,
self-hosted union of error logging, uptime checks, operational events, and
incident coordination that fleets of agents can read from and write back to. In
the Factory composition, Canary is the monitoring half: the ledger that tells
orchestrators what is unhealthy, who already claimed it, and what evidence came
back.

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
- a cold-operator path that lets a new team run its own instance without
  inheriting another operator's app name, dogfood registry, or secrets

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
It also wins only if the instance is boring to own: a fresh operator should be
able to deploy, recover the first admin key, add a service, and prove readback
from public docs and repo-local commands.

## What Canary Is

**A hyper-simple production health service.** One Rust service, one Docker
image, one SQLite database, one deployment target, and a small set of explicit
operator scripts. Complexity has to earn its way in.

**A cold-self-hostable product.** The product must not require the Misty Step
production instance, Phaedrus dogfood data, personal paths, or private
operating lore. Instance configuration belongs outside product code.

**Dual first-class interfaces.** Canary carries one first-class human
interface and one first-class agent interface, and they are not the same
surface: a mobile-friendly web UI for the operator, and MCP/CLI/HTTP for
agents. The human UI is a thin renderer over the same read → claim → annotate
contract agents use — never a parallel semantic API, never a second brain,
never an incident-command war room.

**One semantic contract across surfaces.** HTTP, CLI, MCP, and the web UI
must expose the same loop and authority model. If an agent can read, claim, or
annotate through one surface, the other agent-facing surfaces should not force
raw route trivia or broader keys. The TypeScript SDK is deliberately narrower:
it is an instrumentation surface (capture, check-in, events), not a loop
surface — do not count it in loop-parity claims.

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
- Not a human incident-command suite. The operator web UI is a glanceable
  renderer of the shared contract, not a war room, and it must work in
  private-network deploys (no CDN or third-party callouts).
- Not a semantic workflow engine hidden behind provider-specific agents.
- Not multi-tenant SaaS by default. External-user productization must be
  explicit, scoped, and security-reviewed (see Serving Model below).

Canary may expose MCP tools, SDKs, and CLIs, but those are adapters over the
same contract. They must not become parallel semantic APIs.

## Serving Model

Canary's serving model is intentional, not incidental:

1. **Self-hosted single-tenant binary is the product** — and the competitive
   wedge. One Docker image, one SQLite file, one process, one operator. The
   architecture (SQLite single-writer on one machine) already commits to this;
   the doc says so plainly.

2. **Optional managed hosting is a possible later offering** — running the
   *same* single-tenant binary, one isolated instance per customer (the
   Plausible / PostHog open-core model). This is a convenience/revenue path,
   not a re-architecture: no clustered store, no tenant isolation inside the
   binary. Do not build it now; do not foreclose it. Principle #9 ("design for
   migration, don't build for it") keeps the door open without paying for it.

3. **Multi-tenant SaaS is out by default.** A clustered store plus tenant
   isolation would forfeit the single-binary elegance and put Canary on the
   incumbents' turf (see Competitive Position). Revisiting this is a deliberate,
   security-reviewed decision gated by the external-user security/privacy
   foundation — not an organic drift.

A new reader should be able to answer "self-hosted, managed-later, or SaaS?"
from this section alone, without reading provider configuration or the code.

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
alert reliability, cold-operator deployability, instance/product separation,
stale evidence cleanup, responder safety, fewer overclaimed integrations, and a
cleaner agent handoff from signal to action.

The next product bar is a clean instance run by another operator from the same
verifiably signed OCI artifact and provider-neutral runtime/recovery contract used by every
consumer. Instance topology and provider choices must never leak back into the
product boundary. That artifact is a target state, not a claim about current
release publication.

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

The product direction is not "add every observability feature." The standing
tie-break is **our own fleet first**: prove every loop in anger with the
operator's own agents before polishing the stranger path. In priority order:

- keep the instance itself fast and boring — its own read traffic must never
  make it slow, 500, or restart
- close the read -> claim -> annotate -> release loop through every agent
  surface, and prove it in anger: Canary's own incidents triaged through
  Canary claims, not an external board
- keep releases and versions truthful (tags, releases, registries, and the
  running binary agree)
- make responder context safe enough for arbitrary-agent consumers
- stay substrate-agnostic: releases should become verifiably signed OCI
  artifacts; backup and
  recovery are S3-compatible, restore-proven, and independent of deployment
  topology
- make the operator web UI first-class: mobile-friendly, self-contained,
  degrading gracefully, rendering the shared contract
- then: cold-operator/stranger deployability, OTel GenAI ingest interop, and
  decision-relevant bounded replay — deliberately after the loop is proven

The long-term goal is that an agent can ask one question and act:

> What is unhealthy, who is already working it, what context matters, and what
> is the next safe action?

Canary should be the simplest correct answer.
