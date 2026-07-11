# Integrating Canary into a Next.js app

Canary integrations are API-first. The supported contract is the HTTP API,
`bin/canary`, and MCP; this guide shows a small local adapter rather than a
package dependency.

## 1. Discover, patch, enroll, verify

Prefer the reviewable CLI loop:

```bash
bin/canary integrate discover /path/to/app --production-url https://app.example.com --json
bin/canary integrate plan /path/to/app --service my-app --production-url https://app.example.com --json
bin/canary integrate patch /path/to/app --service my-app --json
bin/canary integrate enroll --service my-app --url https://app.example.com/api/health --project-root /path/to/app --json
bin/canary integrate status /path/to/app --service my-app --production-url https://app.example.com --json
```

`discover` and `plan` report environment-variable names, not values. `patch`
only writes absent or already Canary-owned files and records a reviewable
`.canary/integration.json` receipt. `enroll` redacts the one-time ingest key
unless `--show-secret` is explicitly requested for a secure handoff.

## 2. Configure server and browser boundaries

```dotenv
CANARY_ENDPOINT=https://your-canary.example
CANARY_API_KEY=sk_live_server_ingest_key
NEXT_PUBLIC_CANARY_ENDPOINT=https://your-canary.example
```

`CANARY_API_KEY` must stay server-side. For browser failures, prefer a route in
your application that authenticates to Canary with the server key. If a direct
browser call is unavoidable, use only a constrained ingest-only key and apply
your normal origin, body-size, rate-limit, and redaction controls.

## 3. Server-side request errors

The CLI patch writes `canary.ts` and `instrumentation.ts`. The generated adapter
posts sanitized error fields to the HTTP endpoint and bounds delivery with a
short timeout. A minimal hand-written equivalent is:

```typescript
const endpoint = process.env.CANARY_ENDPOINT ?? "https://your-canary.example";
const apiKey = process.env.CANARY_API_KEY ?? "";

export async function onRequestError(error: unknown, request: { pathname?: string }) {
  if (!endpoint || !apiKey) return;
  const normalized = error instanceof Error
    ? { error_class: error.name || "Error", message: error.message, stack_trace: error.stack }
    : { error_class: "UnknownError", message: String(error), stack_trace: undefined };

  await fetch(`${endpoint.replace(/\/$/, "")}/api/v1/errors`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${apiKey}` },
    body: JSON.stringify({
      service: "my-app",
      environment: process.env.NODE_ENV ?? "production",
      ...normalized,
      context: { pathname: request?.pathname },
      severity: "error",
    }),
    signal: typeof AbortSignal.timeout === "function" ? AbortSignal.timeout(2000) : undefined,
  }).catch(() => undefined);
}
```

Scrub message, stack, and nested context values before sending. Do not put a
server key in a client bundle.

## 4. Browser global errors

Route client errors through an application-owned server endpoint, or use the
same direct HTTP shape with a constrained ingest-only key. Capture both normal
errors and rejected promises:

```typescript
window.addEventListener("error", () => reportBrowserError("BrowserError"));
window.addEventListener("unhandledrejection", () => reportBrowserError("UnhandledRejection"));
```

The generated `app/global-error.tsx` calls the local `canary.ts` adapter. Keep
that adapter's redaction and timeout behavior intact when customizing it.

## 5. Health and non-HTTP check-ins

Add a stable health route for HTTP targets:

```typescript
export function GET() {
  return Response.json({ status: "ok" });
}
```

For cron jobs, workers, desktop sessions, and CLI automation, send check-ins:

```bash
curl -fsS -X POST "$CANARY_ENDPOINT/api/v1/check-ins" \
  -H "Authorization: Bearer $CANARY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"service":"my-app","monitor":"my-app-cron","status":"ok","summary":"nightly job complete"}'
```

Use `alive` for TTL-style freshness and `in_progress`, `ok`, or `error` for
scheduled runs. Send operational events to `/api/v1/events` with the same
scoped server key.

## 6. Verify live coverage

A patch or configured environment is not coverage proof. Verify the target,
query readback, and receipt:

```bash
curl -fsS -H "Authorization: Bearer $CANARY_READ_KEY" \
  "$CANARY_ENDPOINT/api/v1/query?service=my-app&window=1h"
bin/canary integrate status /path/to/app --service my-app --json
```

The status result must show local capture, live health coverage, query
readback, and a current integration receipt before claiming the service is
covered.
