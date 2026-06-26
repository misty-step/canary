# Ship a runnable Canary MCP server

Priority: P1 · Status: pending · Estimate: M

## Goal
Let an agent connect to Canary over MCP (not just shell out to the CLI or hit the HTTP read API), exposing the read + remediation-claims surface as MCP tools — so agent operators get first-class "what's erroring / claim this incident" access from their own MCP client.

## Why now
Habitat-dogfooding surfaced this. `canary mcp-manifest` emits a generated tool
manifest, but there is **no running MCP server** — agents currently shell out to
the CLI or call the HTTP read API. For an agent-operated consumer (Habitat is
run by the Olympus/Argus/Vulcan agents), MCP is the native control surface, and
"is prod ok?" should be one tool call away. Static manifest drift is tracked
separately in #057.

## Oracle
- [ ] An installable MCP server wraps the CLI manifest; an MCP client lists the tools and invokes one read-only/no-op tool end to end.
- [ ] Tool schemas stay **generated from the CLI** (no separate semantic API) — preserves the #036 invariant.
- [ ] A smoke proof covers install + one tool invocation through the wrapper.

## Relationship to existing backlog
ELEVATES the MCP-wrapper requirement currently embedded as one P1 bullet inside #049 ("ship an installable MCP wrapper over the CLI manifest with a smoke proof"); #050's cold-agent readiness proof depends on it. Filed as a focused, standalone deliverable for discoverability — fold back into #049 if you'd rather keep it bundled.
