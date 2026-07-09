# Integrating Canary into a Next.js app

Agents should prefer the reviewable setup loop before hand-editing snippets:

```bash
bin/canary integrate discover /path/to/app --production-url https://app.example.com --json
bin/canary integrate status /path/to/app --service my-app --production-url https://app.example.com --json
bin/canary integrate plan /path/to/app --service my-app --production-url https://app.example.com --json
bin/canary integrate patch /path/to/app --service my-app --json
bin/canary integrate enroll --service my-app --url https://app.example.com/api/health --project-root /path/to/app --json
```

The `discover` and `plan` phases report env var names only. `enroll` redacts the
one-time ingest key by default; use `--show-secret` only for a secure secret
handoff.

`patch` writes a planned `.canary/integration.json`, and
`enroll --project-root` updates the same receipt to verified state with
target/API-key IDs after hosted enrollment. Treat that receipt as reviewable
state for future agents: `integrate status` reads it alongside live Canary
targets, monitors, webhooks, query readback, and dogfood registry evidence.

## Step 1: Install

```bash
npm install @canary-obs/sdk
```

> **Not resolving yet?** `.github/workflows/sdk-publish.yml` builds, tests,
> and publishes this package with npm provenance on every `sdk-v*` tag, but
> the first publish is held pending an operator step (creating the
> `@canary-obs` npm org and adding its publish token — see
> `docs/compatibility-policy.md`). Until that first release lands, build and
> link from source instead:
>
> ```bash
> # From the Canary repo, build the SDK:
> cd clients/typescript && npm install && npm run build
>
> # In your app, link it via file: (adjust the relative path):
> npm install file:../path/to/canary/clients/typescript
> ```

`bin/canary integrate patch` adds `"@canary-obs/sdk": "^1.0.0"` to your
`package.json` dependencies. Until the first tagged release publishes (see the
callout above), replace that version specifier with the `file:` path instead,
or the install will 404.

## Step 2: Add env vars

In `.env.local` and your deployment platform:
```dotenv
CANARY_API_KEY=sk_live_...
CANARY_ENDPOINT=https://your-canary.example
NEXT_PUBLIC_CANARY_ENDPOINT=https://your-canary.example
```

`CANARY_API_KEY` is for server-side ingest. Do not expose raw Canary API keys in
browser bundles. Browser-side ingest should use service-bound public DSNs once
that surface lands; until then, route client errors through an application-owned
server endpoint or accept the risk explicitly with a constrained ingest-only key.

## Step 3: Initialize in `instrumentation.ts` (server-side)

```typescript
import { initCanary } from "@canary-obs/sdk";
export { onRequestError } from "@canary-obs/sdk/nextjs";

export function register() {
  initCanary({
    endpoint: process.env.CANARY_ENDPOINT ?? "https://your-canary.example",
    apiKey: process.env.CANARY_API_KEY ?? "",
    service: "my-app",
    environment: process.env.NODE_ENV ?? "production",
  });
}
```

This captures all server-side errors automatically via `onRequestError`.
The SDK scrubs common PII by default; Canary also applies server-side redaction
before persistence.

## Step 4: Client-side error boundaries

The SDK uses module-level state. Server-side `initCanary()` runs in a separate
bundle from client components. Client-side capture requires a separate
`initCanary()` call in the browser.

### `app/providers.tsx`

```typescript
"use client";
import { useEffect } from "react";
import { initCanary } from "@canary-obs/sdk";

export function CanaryProvider({ children }: { children: React.ReactNode }) {
  useEffect(() => {
    initCanary({
      endpoint: "/api/canary",
      apiKey: "relay", // Non-secret sentinel; the relay owns the real key.
      service: "my-app",
    });
  }, []);
  return <>{children}</>;
}
```

Wrap your root layout with `<CanaryProvider>`.

### `app/api/canary/api/v1/errors/route.ts`

```typescript
export async function POST(request: Request) {
  const payload = await request.json();
  const { service: _service, environment: _environment, ...event } = payload;

  const response = await fetch(
    `${process.env.CANARY_ENDPOINT ?? "https://your-canary.example"}/api/v1/errors`,
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${process.env.CANARY_API_KEY ?? ""}`,
      },
      body: JSON.stringify({
        ...event,
        service: "my-app",
        environment: process.env.NODE_ENV ?? "production",
      }),
    }
  );

  if (!response.ok) {
    return Response.json({ ok: false }, { status: 502 });
  }

  return Response.json(await response.json(), { status: 202 });
}
```

The relay path mirrors the SDK ingest suffix, so `endpoint: "/api/canary"`
causes browser captures to post to `/api/canary/api/v1/errors`. Keep the raw
Canary API key in server-only environment variables, and apply the same body
size, origin, and rate-limit controls you use for other public write endpoints.

### `app/global-error.tsx`

```typescript
"use client";
import { useEffect } from "react";
import { captureNextGlobalError } from "@canary-obs/sdk/nextjs";

export default function GlobalError({ error }: { error: Error }) {
  useEffect(() => {
    captureNextGlobalError(error);
  }, [error]);

  return (
    <html>
      <body>
        <h1>Something went wrong</h1>
      </body>
    </html>
  );
}
```

### `app/**/error.tsx`

```typescript
"use client";
import { useEffect } from "react";
import { captureNextErrorBoundary } from "@canary-obs/sdk/nextjs";

export default function Error({ error }: { error: Error }) {
  useEffect(() => {
    captureNextErrorBoundary(error);
  }, [error]);

  return <h1>Something went wrong</h1>;
}
```

For global browser observers in a client provider:

```typescript
"use client";
import { useEffect } from "react";
import { installBrowserErrorObservers } from "@canary-obs/sdk/nextjs";

export function CanaryBrowserObservers() {
  useEffect(() => installBrowserErrorObservers(), []);
  return null;
}
```

For Sentry migrations or dual-write windows:

```typescript
import * as Sentry from "@sentry/nextjs";
import { captureSentryEvent } from "@canary-obs/sdk/nextjs";

Sentry.addEventProcessor((event) => {
  void captureSentryEvent(event);
  return event;
});
```

For a generated health route:

```typescript
import { canaryHealthResponse } from "@canary-obs/sdk/nextjs";

export function GET() {
  return canaryHealthResponse();
}
```

## Step 5: Manual capture (server-side)

```typescript
import { captureException, captureMessage } from "@canary-obs/sdk";

try {
  await riskyOperation();
} catch (err) {
  captureException(err, {
    severity: "error",
    context: { userId: user.id, route: "/api/sessions" },
  });
}

captureMessage("deployment complete", { severity: "info" });
```

## Step 6: Non-HTTP monitor check-ins

For cron jobs, workers, desktop active sessions, and CLI automations, create a
Canary monitor from `bin/canary integrate plan --json`, then report check-ins:

```typescript
import { checkIn } from "@canary-obs/sdk";

await checkIn({
  monitor: "my-app-cron",
  status: "ok",
  summary: "nightly job complete",
  context: { duration_ms: 1200 },
});
```

Use `status: "alive"` for TTL-style worker/session freshness, and
`"in_progress"`, `"ok"`, or `"error"` for scheduled runs.

## Step 7: Verify

```bash
curl -H "Authorization: Bearer $CANARY_API_KEY" \
  "$CANARY_ENDPOINT/api/v1/query?service=my-app&window=1h"
```

## Dual-write with Sentry

During migration, call both:

```typescript
import { captureException } from "@canary-obs/sdk";
import * as Sentry from "@sentry/nextjs";

export function reportError(error: Error, context?: Record<string, unknown>) {
  Sentry.captureException(error, { extra: context });
  captureException(error, { context });
}
```

Once confident, remove `@sentry/nextjs` and simplify to Canary only.
