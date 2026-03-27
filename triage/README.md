# Canary Triage

Companion Phoenix service for Canary webhooks and GitHub issue synthesis.

Use the repo-root workflow for a full checkout:

```bash
./bin/bootstrap
./bin/validate
```

For triage-only work:

```bash
cd triage
mix setup
mix phx.server
```

Triage deploys from `triage/`, not the repo root.
