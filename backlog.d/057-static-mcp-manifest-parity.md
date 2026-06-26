# Regenerate and gate the static MCP manifest snapshot

Priority: P1 · Status: ready · Estimate: S

## Goal
Make the checked-in `priv/mcp/canary-cli-tools.json` snapshot either match
`bin/canary mcp-manifest` exactly or stop existing as a parallel source of
truth, so agents never read a stale MCP tool contract.

## Why now
PR #172 updated the live `canary_summary` MCP description and surfaced the
final #047 SLI trajectory data, but left the static snapshot stale and ungated.
A live check on 2026-06-26 showed:

- checked-in `priv/mcp/canary-cli-tools.json`: 13 tools
- generated `bin/canary mcp-manifest`: 23 tools

Dagger currently validates the generated manifest shape during the
doctor/MCP smoke, but it does not diff the checked-in snapshot. The stale file
is agent-facing enough to become misleading, especially before #052 ships a
runnable MCP server.

## Oracle
- [ ] Given `bin/canary mcp-manifest` runs in a clean checkout, then the
      checked-in `priv/mcp/canary-cli-tools.json` is byte-for-byte equivalent
      after stable JSON formatting, or the file is removed and all references
      point to the generator.
- [ ] Given `canary-cli::tool_manifest()` changes, then the canonical gate fails
      if a retained static snapshot was not regenerated.
- [ ] Given docs or backlog tickets mention MCP tool counts, then they do not
      hardcode stale counts; they either derive from the generator or describe
      the contract without a count.

## Verification System
- Claim: the static MCP manifest cannot drift from the CLI-generated manifest.
- Falsifier: `bin/canary mcp-manifest` emits a different tool set than
  `priv/mcp/canary-cli-tools.json` while `./bin/validate` still passes.
- Driver: a focused CLI fixture/parity test plus the existing Dagger MCP smoke.
- Grader: a stable JSON diff over generated vs checked-in manifest is empty, or
  no checked-in snapshot remains to drift.
- Evidence packet: test transcript plus the generated/diff output in the PR.
- Cadence: every repo gate when the snapshot is retained.

## Relationship to existing backlog
Follow-up to #047 child #6 and prerequisite hygiene for #050/#052. This does
not ship the runnable MCP server; #052 still owns server installation and one
client invocation.
