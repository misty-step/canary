# @canary-obs/sdk

Error reporting and check-in SDK for [Canary](https://github.com/misty-step/canary),
a self-hosted production health ledger for errors, uptime, incidents, and
webhooks.

## Install

Not yet published to npm — the `@canary-obs` org and an npm publish token are
operator setup steps that haven't happened yet
([`sdk-publish.yml`](../../.github/workflows/sdk-publish.yml) is ready and
gated on the `NPM_TOKEN` secret; see
[`docs/compatibility-policy.md`](https://github.com/misty-step/canary/blob/master/docs/compatibility-policy.md)
for the publish plan). Until then, build and link from source:

```bash
# From the Canary repo, build the SDK:
git clone https://github.com/misty-step/canary.git
cd canary/clients/typescript && npm install && npm run build

# In your app, link it via file: (adjust the relative path):
npm install file:../path/to/canary/clients/typescript
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
