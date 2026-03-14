# Integrating Canary into Volume

Surgical integration: dual-write to Sentry + Canary during parallel period, then remove Sentry.

## Step 1: Copy client into Volume

```bash
cp clients/typescript/canary.ts ~/Development/volume/src/lib/canary.ts
```

## Step 2: Add env var

In `.env.local` and Vercel environment variables:
```
CANARY_API_KEY=sk_live_...
NEXT_PUBLIC_CANARY_ENDPOINT=https://canary-obs.fly.dev
```

Note: API key is server-only (no `NEXT_PUBLIC_` prefix) to avoid exposing it in client bundles.

## Step 3: Initialize Canary client

Create `src/lib/canary-instance.ts`:

```typescript
import { Canary } from "./canary";

// Server-side only — API key must not leak to client bundles
export const canary = new Canary({
  endpoint: process.env.NEXT_PUBLIC_CANARY_ENDPOINT ?? "https://canary-obs.fly.dev",
  apiKey: process.env.CANARY_API_KEY ?? "",
  service: "volume",
  environment: process.env.NODE_ENV ?? "production",
  enabled: !!process.env.CANARY_API_KEY,
});
```

## Step 4: Dual-write in reportError

In `src/lib/analytics.ts`, modify `reportError`:

```typescript
import { canary } from "./canary-instance";

export function reportError(
  error: Error,
  context?: Record<string, unknown>
): void {
  const sanitizedContext = context
    ? sanitizeEventProperties(context)
    : undefined;

  // Sentry (existing)
  if (isSentryEnabled()) {
    try {
      Sentry.captureException(error, { extra: sanitizedContext });
    } catch {
      // Never break user flow
    }
  }

  // Canary (new — parallel write)
  canary.capture(error, {
    context: sanitizedContext,
  });
}
```

## Step 5: Verify

```bash
# Check that errors flow to Canary
curl -H "Authorization: Bearer $CANARY_API_KEY" \
  "https://canary-obs.fly.dev/api/v1/query?service=volume&window=1h"
```

## Step 6: After parallel period — remove Sentry

Once confident in Canary's error capture:

1. Remove `@sentry/nextjs` and `@sentry/cli` from `package.json`
2. Delete `sentry.client.config.ts`, `sentry.server.config.ts`, `sentry.edge.config.ts`
3. Remove `withSentryConfig()` from `next.config.ts`
4. Remove Sentry CSP entries
5. Remove `src/lib/sentry.ts`
6. Simplify `reportError` to only call `canary.capture()`
7. Remove Sentry env vars from Vercel
