# Prove live integration behavior in the gate

Priority: P0
Status: ready
Estimate: L

## Goal
Make the canonical gate prove Canary's production-image write/readback behavior across SDK ingest, health, webhooks, CLI/doctor, MCP manifest, and integration receipts instead of only proving boot readiness.

## Oracle
- [ ] Dagger strict runs a production-image integration harness that mints scoped keys, creates a target/monitor/webhook, ingests a synthetic error through the TypeScript SDK or generated fixture, and reads it back through query/report/timeline/error detail.
- [ ] The harness verifies webhook delivery ledger rows, stable delivery IDs, retry/diagnostic visibility, and signed test delivery behavior.
- [ ] The harness runs `bin/canary doctor --json`, validates worker readiness from `/readyz`, and fails if doctor reports stale placeholders for shipped readiness features.
- [ ] The harness validates the MCP manifest schemas, not only tool names.
- [ ] SDK/server payload contract tests exercise the real Rust ingest handler instead of only mocked `fetch` calls.
- [ ] `bin/canary-write-path-rehearsal` remains useful for live Fly evidence, but the PR gate owns deterministic local proof.

## Children
1. Convert the existing production-image smoke from health-only to write/readback integration proof.
2. Add SDK-to-Rust ingest contract fixtures and run them from strict.
3. Add webhook ledger proof and signed delivery validation to the production-image harness.
4. Make `doctor` consume worker readiness now that #034 has shipped.
5. Validate MCP argument schemas and command envelopes in strict.
6. Record the harness receipt path in docs and backlog closeout guidance.

## Notes
- Evidence: `dagger/src/index.ts` currently curls `/healthz` and `/readyz`; `test/bin/canary_write_path_rehearsal_test.sh` stubs external commands; `clients/typescript/test/client.test.ts` mocks `globalThis.fetch`; `crates/canary-cli/src/lib.rs` still reports `worker_readiness` unavailable because #034 had not landed when the doctor code was written.
- Verification lane found #038's intent is shipped, but its broad coverage oracle is not yet owned by the gate.
