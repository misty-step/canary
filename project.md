# Project: Canary

## Vision
Self-hosted observability for agent-driven infrastructure — error tracking and uptime monitoring in one service.

**North Star:** Every project reports errors and health to one Canary instance. Agents query it naturally. Operators see everything at a glance.
**Target User:** Solo developer running multiple services on Fly.io with AI agent-assisted development.
**Key Differentiators:** Single-binary SQLite deployment. Combined error tracking + uptime monitoring. REST API designed for AI agents. ~3,500 LOC total.

## Principles
- **Deep modules, simple interfaces.** One function to ingest an error. One function to transition state. Hide complexity behind narrow APIs.
- **Server does the thinking.** Clients send raw events. Canary groups, deduplicates, correlates, summarizes. SDKs are logger handlers, not analytics libraries.
- **Deterministic over clever.** Summaries are templates, not LLM calls. State transitions are pure functions. Grouping is SHA256, not embeddings.
- **Single-writer SQLite.** One Repo for writes (pool_size: 1). ReadRepo for queries. Litestream for backup. No Postgres, no Redis, no Kafka.
- **Webhooks are the integration layer.** Side effects flow through Oban jobs with circuit breakers and cooldown. Never on the request path.

## Philosophy
- Build for one user (me), not a market. Define interfaces from first principles, not by copying Sentry.
- Code is a liability. Every line fights for its life. ~3,500 LOC for error tracking + uptime monitoring + webhook delivery + health check state machine is the bar.
- Agent-first, not dashboard-first. The API is the primary interface. The dashboard is a convenience, not the product.
- OTP is the runtime. Supervisors, GenServers, and ETS are the tools. No external queues, no external caches.

## Domain Glossary

| Term | Definition |
|------|-----------|
| Target | A URL that Canary health-checks on a schedule |
| Target State | Pure state machine: unknown → up/degraded/down/paused/flapping |
| Error Group | Errors clustered by group_hash (SHA256 of fingerprint, stack trace, or message template) |
| Probe | A single HTTP health check execution against a target |
| Webhook | Registered HTTP callback fired on state transitions and new error classes |
| Triage | Companion service that receives webhooks and creates GitHub issues via LLM synthesis |

## Quality Bar
- [ ] All tests pass (`mix test`)
- [ ] Credo clean (`mix credo --strict`)
- [ ] Dialyzer clean (`mix dialyzer`)
- [ ] No module exceeds 500 LOC
- [ ] RFC 9457 Problem Details for all error responses
- [ ] Deterministic summaries (no LLM on request path)

## Patterns to Follow

### Deep Module (Ingest)
```elixir
# One public function, complex internals
def ingest(params) do
  with {:ok, attrs} <- validate(params),
       {:ok, group_hash} <- Grouping.compute_group_hash(attrs),
       {:ok, error} <- persist(attrs, group_hash),
       :ok <- maybe_enqueue_webhook(error, group_hash) do
    {:ok, error}
  end
end
```

### Pure State Machine
```elixir
# No side effects. Returns {new_state, effects_list}.
def transition(current_state, event, counters, opts) do
  # Table-driven logic, fully testable
end
```

### Logger Handler SDK
```elixir
# SDK is a tap on existing :logger, not a new API
CanarySdk.attach(endpoint: "...", api_key: "...", service: "my-app")
# That's it. Errors flow automatically.
```

## Lessons Learned

| Decision | Outcome | Lesson |
|----------|---------|--------|
| Cast custom string PKs via changeset | Silently dropped id field (6 bugs) | Set id on struct, not via cast |
| Oban.Migrations.SQLite with pool_size:1 | Race condition on boot | Manual Ecto migration with raw SQL |
| Req + Finch + connect_options | Incompatible options error | Use :receive_timeout, not :connect_options |
| GHA cron for dead-man's-switch | Worked perfectly, zero cost | External watchdog via CI is ideal for single-service |

---
*Last updated: 2026-03-15*
*Updated during: /groom session*
