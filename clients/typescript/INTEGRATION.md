# Integrating Canary into a Next.js app

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
NEXT_PUBLIC_CANARY_API_KEY=sk_live_...
```

`CANARY_API_KEY` is for server-side. `NEXT_PUBLIC_` variants are for client-side
error boundaries (write-only key — safe to expose in browser bundles).

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
    scrubPii: true,
  });
}
```

This captures all server-side errors automatically via `onRequestError`.

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
      endpoint: process.env.NEXT_PUBLIC_CANARY_ENDPOINT ?? "",
      apiKey: process.env.NEXT_PUBLIC_CANARY_API_KEY ?? "",
      service: "my-app",
    });
  }, []);
  return <>{children}</>;
}
```

Wrap your root layout with `<CanaryProvider>`.

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
