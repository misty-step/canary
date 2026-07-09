# @canary-obs/sdk

Error reporting and check-in SDK for [Canary](https://github.com/misty-step/canary),
a self-hosted production health ledger for errors, uptime, incidents, and
webhooks.

## Install

```bash
npm install @canary-obs/sdk
```

## Usage

```typescript
import { initCanary, captureException } from "@canary-obs/sdk";

initCanary({
  endpoint: "https://your-canary.example",
  apiKey: process.env.CANARY_API_KEY!,
  service: "my-app",
  environment: process.env.NODE_ENV ?? "production",
});

try {
  await riskyOperation();
} catch (err) {
  captureException(err, { severity: "error" });
}
```

Next.js apps get a dedicated subpath (`@canary-obs/sdk/nextjs`) with
`onRequestError`, error-boundary helpers, browser error observers, a
Sentry-event bridge, and a health-check route helper.

See [`INTEGRATION.md`](./INTEGRATION.md) for the full Next.js setup (server
init, client-side error boundaries, non-HTTP monitor check-ins, Sentry
dual-write migration) and the compatibility guarantees in
[`docs/compatibility-policy.md`](https://github.com/misty-step/canary/blob/master/docs/compatibility-policy.md).

## License

MIT
