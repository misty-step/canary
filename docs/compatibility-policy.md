# Compatibility Policy

Canary's external contracts are stable by version. Breaking changes
require explicit migration steps and are documented here.

## OpenAPI (`GET /api/v1/openapi.json`)

- The `info.version` field tracks the API version.
- Route paths, request/response shapes, and status codes are stable
  within a major version.
- The `info.x-agent-guide` extension embeds the canonical replay guide;
  changes to it are documented in the spec diff.
- Source: `priv/openapi/` (committed JSON fragments).

## CLI (`bin/canary`)

- The JSON envelope `schema_version` field is stable at `1`.
- Commands that change shape get a new envelope version or a documented
  additive field.
- Text output is terse and may change format; JSON output is the stable
  contract.
- The MCP manifest (`bin/canary mcp-manifest`) is gated against the
  runtime tool list so it cannot drift from the CLI.

## MCP (`bin/canary mcp-server`)

- The MCP protocol version is pinned in `crates/canary-cli/src/main.rs`.
- Tool names and `inputSchema` are derived from the CLI manifest;
  changes are additive (new tools) or versioned (renamed tools require
  a protocol bump).
- Tool call results return the CLI JSON envelope as
  `structuredContent`; runtime failures return `isError: true`.

## TypeScript SDK (`clients/typescript/`)

- The SDK is not yet published to npm (tracked in #051).
- Until npm publish ships, install from source via `file:` linking
  (see `clients/typescript/INTEGRATION.md`).
- The package exports `@canary-obs/sdk` (main) and
  `@canary-obs/sdk/nextjs` (Next.js integration); these entry points
  are stable.
- Breaking changes to public exports are versioned (semver bump).

## SQLite Schema Migrations

- Migrations are forward-only. `Store::migrate` stamps `user_version`
  after applying missing migrations.
- The migration set fails closed on partial existing schemas before
  stamping (see `CLAUDE.md` footgun: "Schema ownership").
- There is no automated schema rollback. See
  [`docs/upgrade-and-rollback.md`](upgrade-and-rollback.md) for the
  restore-based rollback procedure.

## Webhook Payloads

- The webhook contract version is pinned at `x-webhook-version: 1`.
- Payload shapes are stable product contracts consumed by downstream
  responders (e.g. Bitterblossom).
- The incident event payload shape is pinned by conformance tests in
  `crates/canary-store/src/incidents.rs` (see #080).
- Bumping the webhook version is a breaking change requiring lockstep
  consumer migration. The `subject` + `schema_version:1` form is a
  future coordinated migration, not the current contract.

## API Key Scopes

- Scopes (`ingest-only`, `read-only`, `responder-write`, `admin`) are
  stable wire values. See `docs/api-key-rotation.md`.
- Adding a new scope is additive; removing or renaming a scope is a
  breaking change.

## Error Responses (RFC 9457)

- All error responses use RFC 9457 Problem Details with stable
  `type` URIs, `title` strings, and `code` values.
- Adding new problem codes is additive; removing or renaming codes is
  a breaking change.
- Source: `crates/canary-http/src/problem_details.rs`.
