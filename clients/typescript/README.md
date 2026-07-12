# TypeScript source reference

This directory is a **private, source-only reference** for TypeScript error,
check-in, event, and Next.js instrumentation adapters over Canary's HTTP API.
It is not a published package and is not an integration dependency.

Canary's supported integration contract is the HTTP API plus the `bin/canary`
CLI and MCP surfaces. Use `bin/canary integrate plan` or
`bin/canary integrate patch` to generate a local adapter for an application;
review that code before deploying it. Keep server-only ingest keys in server
environment variables. Browser code should use an application-owned relay or
a constrained ingest-only key.

## Local development

The package is retained so contributors can test the reference implementation:

```bash
npm ci
npm run typecheck
npm test
npm run build
```

`private: true` is intentional. No npm organization, registry token, publish
tag, or package installation is part of Canary's product contract.

## HTTP contract

A server-side error adapter posts JSON to `/api/v1/errors` with a scoped
`Authorization: Bearer <ingest-key>` header. Non-HTTP runtimes report monitor
state to `/api/v1/check-ins` and operational events to `/api/v1/events`. The
CLI and MCP surfaces provide enrollment, verification, query, timeline, and
incident workflows over the same service contract.

See [`INTEGRATION.md`](./INTEGRATION.md) for a copyable API-first Next.js
walkthrough and [`docs/compatibility-policy.md`](../../docs/compatibility-policy.md)
for the stable surface policy.

## License

MIT
