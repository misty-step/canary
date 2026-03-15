# Integrating Canary into a Next.js app

## Step 1: Install

```bash
npm install @canary-obs/sdk
```

## Step 2: Add env vars

In `.env.local` and your deployment platform:
```
CANARY_API_KEY=sk_live_...
CANARY_ENDPOINT=https://canary-obs.fly.dev
```

API key is server-only — no `NEXT_PUBLIC_` prefix.

## Step 3: Initialize in `instrumentation.ts`

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

In `app/global-error.tsx` or any `error.tsx`:

```typescript
"use client";
import { captureException } from "@canary-obs/sdk";

export default function GlobalError({ error }: { error: Error }) {
  captureException(error);
  return <h1>Something went wrong</h1>;
}
```

## Step 5: Manual capture

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
