# Vision

## The Problem

Observability tooling was built for humans. Dashboards, alert fatigue, click-through workflows, context-switching between Sentry and Uptime Robot and PagerDuty. The primary consumer of production health data is increasingly AI agents — and they need something fundamentally different.

Agents don't need pretty graphs. They need structured, bounded, pre-aggregated data they can reason over in a single context window. They need natural-language summaries they can include in reports. They need webhook events they can act on autonomously.

## The Bet

The agent-infrastructure feedback loop is the next platform shift:

1. Agent detects anomaly (via Canary webhook or periodic query)
2. Agent queries for context (error groups, check history, stack traces)
3. Agent diagnoses and either fixes automatically or files a structured report
4. Agent verifies the fix landed (query again, check health status)

This loop doesn't need a dashboard. It needs an API that speaks the agent's language.

## What Canary Is

A single, self-hosted service that replaces the core of Sentry (error capture) and Uptime Robot (health monitoring) with an API designed for AI agents.

**One service.** One Docker image, one SQLite file, one config. No microservices, no message queues, no external dependencies.

**Agent-first.** Every response includes a `summary` field. Responses are bounded and pre-aggregated. Error classes are grouped automatically. Health state is a finite state machine with clear semantics.

**Broadcast, don't prescribe.** Generic webhooks with HMAC signing. Consumers define their own behavior. No opinionated integrations with GitHub, Slack, Discord, or anything else.

## What Canary Is Not

- Not a replacement for distributed tracing, APM, or log aggregation
- Not a dashboard (agents are the UI)
- Not multi-tenant (internal tool, single-org)
- Not an MCP server (API + CLI + skill files are sufficient)

## Where It's Going

### Near Term (v1 — current)
Single-region health checking, error ingestion, query API, webhook broadcasting. Proves the concept with our own infrastructure.

### Medium Term (v2)
- Multi-region health probing (Fly.io workers in multiple regions, consensus-based down detection)
- Push-based monitoring (heartbeat endpoint for cron jobs)
- Postgres migration (Ecto makes this a config change)
- Logger backend (automatic capture with noise filtering)

### Long Term
Canary becomes the canonical interface between AI agents and production infrastructure. Not by adding features, but by being the simplest correct answer to: "what's happening in production right now?"
