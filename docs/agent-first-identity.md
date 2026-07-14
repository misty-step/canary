# Agent-First Identity

Canary's identity is simple: production health for agents.

The system exists so many agents can safely watch deployed applications, wake
up on important events, coordinate ownership, query context, and write back
evidence without a dashboard or a human-only incident process.

## Core Philosophy

Agents are not dashboard users. They are readers, writers, and responders.

They need stable contracts, compact context, replayable timelines, and explicit
ownership state. They do not need charts, tabs, or a pile of integrations that
hide the actual event stream.

Canary should feel like a small command center:

- It records what happened.
- It decides enough to make the first response cheap.
- It wakes agents without making webhooks authoritative.
- It lets agents claim work before acting.
- It preserves the evidence trail after the agent goes away.

## Design Laws

### 1. Agents are the primary users

Every public surface should work well in a transcript, CLI JSON envelope, API
response, or MCP tool result. Human operators inspect those same surfaces.

### 2. The timeline is truth

Notifications can be missed, duplicated, retried, or delivered late. Agents
recover by replaying timeline state from a persisted cursor.

### 3. Webhooks wake; they do not decide

A signed webhook tells a downstream agent to wake up. It must not be treated as
the full source of context. The receiver verifies the signature, deduplicates
the delivery id, then queries Canary.

### 4. Claims prevent duplicate agents

Before an agent investigates or repairs an incident, error group, target, or
monitor, it creates a remediation claim with an idempotency key. Active claims
are the coordination state.

### 5. Every high-level read returns one next action

Reports, doctor verdicts, integration status, and value receipts should avoid
menus of vague suggestions. The default output is one concrete next action and
links or ids for drill-down.

### 6. Summaries are deterministic

Canary may feed LLMs, but it does not put an LLM on the request path. Summaries
are templates over typed state so agents can trust them as stable evidence.

### 7. Integration is not installation

Adding an SDK or target is not enough. A service is covered only when live
readback proves health, ingest, query, and the relevant target, monitor,
webhook, or receipt state.

### 8. Canary does not fix production

Canary records, correlates, routes, and coordinates. Repository mutation,
issue creation, LLM triage, deployment rollback, and customer communication
live downstream.

### 9. The product stays small

If a feature needs a dashboard, distributed trace store, human war room, or
full log platform to make sense, it probably belongs outside Canary. Canary
should integrate by durable events and read models instead.

### 10. Self-watch is a first-class consumer

Canary must prove itself the same way it asks other services to prove
coverage: external witness, readback, receipts, worker readiness, and explicit
operator verdicts.

## Agent UX Contract

The ideal agent flow is:

1. Wake from a signed webhook or scheduled poll.
2. Read `canary_doctor` or `GET /api/v1/report?window=1h`.
3. If there is a subject to work, create a remediation claim.
4. Read the specific subject: incident detail, error group, target, monitor,
   timeline, annotations, and delivery diagnostics as needed.
5. Act outside Canary.
6. Write annotations, claim transitions, evidence links, and final status.
7. Persist the timeline cursor and go idle.

The CLI/API/MCP surface is good when an agent can complete that loop without
knowing internal table names, raw route trivia, or chat-only instructions.

## Current Useful Surfaces

- `bin/canary doctor --json`: current trust verdict for Canary itself.
- `bin/canary dogfood value --service <name> --json`: per-service value
  receipt with coverage, live readback, stale evidence, and next action.
- `GET /api/v1/report`: bounded current state for agents without saved cursors.
- `GET /api/v1/timeline`: durable replay after report or webhook wake-up.
- `GET /api/v1/incidents/{id}`: incident-level entrypoint with correlated
  signals and annotations.
- `GET /api/v1/claims/active`: fleet-wide "who is working what" before
  claiming or planning remediation.
- `POST /api/v1/claims`: ownership before triage or repair.
- `POST /api/v1/annotations`: evidence and decisions after work.
- `bin/canary integrate status/plan/patch/enroll`: setup loop for deployed
  services.
- `bin/canary mcp-manifest`: machine-readable CLI tool contract snapshot.
- `bin/canary mcp-server`: stdio MCP server over the generated CLI tool
  contract.

## Value To Consuming Applications

For a consuming application, Canary provides:

- one place to write production errors and operational events
- one place to prove HTTP uptime or non-HTTP liveness
- one durable incident and timeline model for downstream agents
- one agent-readable readback contract instead of several human dashboards
- one coordination primitive to prevent duplicate remediation agents
- one integration receipt that says whether coverage is real or stale

The value is strongest when the consuming app has agents ready to listen and
respond. Without agents, Canary is still a useful small monitor, but it is not
trying to beat mature human-first observability suites on breadth.

## Anti-Patterns

- Adding a human dashboard because a response shape is hard to read.
- Treating webhook delivery as proof without timeline replay.
- Marking a service covered because env vars or code paths exist.
- Letting agents mutate repos or infrastructure through Canary-owned routes.
- Adding broad logs, traces, metrics, or LLM eval features before the incident
  and claim loop is excellent.
- Returning several plausible next actions when one safe action can be named.
- Allowing stale dogfood evidence to masquerade as current value.

## Product Taste

Canary should feel calm, terse, and exact.

Good output:

- names the unhealthy subject
- says whether an agent already owns it
- gives the recent facts that matter
- links to the durable ids needed for replay
- names one next action

Bad output:

- dumps raw telemetry
- asks the agent to inspect a dashboard
- hides stale evidence
- wakes multiple agents with no claim path
- creates incidents without enough context to act
- blends human process with agent coordination

The project should keep choosing the smaller primitive that makes agents more
effective.
