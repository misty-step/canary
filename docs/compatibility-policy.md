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
- Integration command response payloads (`integrate discover`, `integrate plan`, and `integrate patch`) expose `response.schema_version: 2` inside the stable CLI envelope. Compared with v1, `signals.canary_sdk_dependency` is now `signals.legacy_canary_package_dependency` and the `sdk_dependency` action is now `legacy_package_dependency`; consumers must branch on the response version before reading these fields.
- The MCP manifest (`bin/canary mcp-manifest`) is gated against the
  runtime tool list so it cannot drift from the CLI.

## MCP (`bin/canary mcp-server`)

- The MCP protocol version is pinned in `crates/canary-cli/src/main.rs`.
- Tool names and `inputSchema` are derived from the CLI manifest;
  changes are additive (new tools) or versioned (renamed tools require
  a protocol bump).
- Tool call results return the CLI JSON envelope as
  `structuredContent`; runtime failures return `isError: true`.

## TypeScript source reference (`clients/typescript/`)

- This directory is private source for local TypeScript/Next.js adapters over
  Canary's HTTP API. It is not a published package or a supported dependency.
- No npm organization, registry token, publish tag, or package-install contract
  exists for this source reference. The directory's `private: true` metadata is
  intentional.
- Breaking changes to the HTTP request/response contract are versioned in the
  OpenAPI document. Local adapters should send errors to `/api/v1/errors`,
  check-ins to `/api/v1/check-ins`, and events to `/api/v1/events` with scoped
  API keys.
- `bin/canary integrate` and MCP provide the supported enrollment, verification,
  query, timeline, and incident workflows. Generated application adapters must
  be reviewed for key scope, redaction, and timeout behavior.

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
