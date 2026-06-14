# Integrating Canary into a Next.js app

Agents should prefer the reviewable setup loop before hand-editing snippets:

```bash
bin/canary integrate discover /path/to/app --production-url https://app.example.com --json
bin/canary integrate plan /path/to/app --service my-app --production-url https://app.example.com --json
bin/canary integrate patch /path/to/app --service my-app --json
bin/canary integrate enroll --service my-app --url https://app.example.com/api/health --json
```

The `discover` and `plan` phases report env var names only. `enroll` redacts the
one-time ingest key by default; use `--show-secret` only for a secure secret
handoff.

## Step 1: Install

```bash
npm install @canary-obs/sdk
```

## Step 2: Add env vars

In `.env.local` and your deployment platform:
```dotenv
CANARY_API_KEY=sk_live_...
CANARY_ENDPOINT=https://canary-obs.fly.dev
NEXT_PUBLIC_CANARY_ENDPOINT=https://canary-obs.fly.dev
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
    endpoint: process.env.CANARY_ENDPOINT ?? "https://canary-obs.fly.dev",
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
    `${process.env.CANARY_ENDPOINT ?? "https://canary-obs.fly.dev"}/api/v1/errors`,
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
import { captureException } from "@canary-obs/sdk";

export default function GlobalError({ error }: { error: Error }) {
  useEffect(() => {
    captureException(error);
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
import { captureException } from "@canary-obs/sdk";

export default function Error({ error }: { error: Error }) {
  useEffect(() => {
    captureException(error);
  }, [error]);

  return <h1>Something went wrong</h1>;
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

## Step 6: Verify

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
