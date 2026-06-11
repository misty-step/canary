# Ship an agent-native Canary inspection surface

Priority: high
Status: done
Estimate: L

## Goal
Give Codex, Claude, and other agents one stable local interface for inspecting Canary status, errors, incidents, uptime, targets, monitors, timelines, and dogfood coverage.

## Oracle
- [x] A Rust CLI entrypoint, named consistently with the repo, supports `summary`, `services`, `errors <service>`, `incidents`, `timeline`, `targets`, `monitors`, `dogfood audit`, and `doctor` commands.
- [x] Every command has `--json` output with stable schemas and concise text output suitable for an agent transcript.
- [x] The CLI reads `CANARY_ENDPOINT` and read/admin scoped keys from env or a local config file, redacts secrets in diagnostics, and fails closed on missing scope.
- [x] `doctor` reports API reachability, key scope, current global summary, unhealthy services, recent high-volume error groups, open incidents, worker readiness once #034 lands, and registry coverage gaps once #035 lands.
- [x] A minimal MCP server or generated MCP tool manifest is designed from the CLI schemas after the CLI stabilizes; it does not introduce a separate semantic API.
- [x] Tests cover response parsing with fixture JSON for report, query, incidents, timeline, targets, monitors, and dogfood inventory.
- [x] `./bin/validate --fast` is green.

## Completion evidence
- `cargo test -p canary-cli --locked` passed with fixture-backed parser coverage.
- `bin/canary summary --window 1h`, `bin/canary targets`, `bin/canary dogfood audit`, and `bin/canary doctor` were exercised against the deployed `canary-obs` API.
- `./bin/validate --fast` passed.

## Notes
**Why:** User watchability request. Canary removed its human dashboard intentionally, but `curl | jq` is not a sufficient first-class agent/operator experience.

**2026-06-11 evidence.** The current API already exposes the primitives: `/api/v1/report`, `/api/v1/status`, `/api/v1/query`, `/api/v1/timeline`, `/api/v1/incidents`, `/api/v1/targets`, `/api/v1/monitors`, `/api/v1/webhook-deliveries`, and `/metrics`. The missing product surface is the stable local agent affordance over those routes.

**Shape.**
```bash
canary summary --window 24h
canary services --state unhealthy --json
canary errors chrondle --window 24h
canary incidents --open
canary dogfood audit --strict
canary doctor
```

**Responder-boundary check.** The CLI reads and summarizes Canary. It may write annotations only through explicit subcommands; it does not decide or perform downstream remediation.
