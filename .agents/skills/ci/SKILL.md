---
name: ci
description: |
  Audit a repo's CI gates, strengthen what is weak, then drive the pipeline
  green. Owns confidence in correctness — lint, types, tests, coverage,
  secrets. Dagger is the canonical pipeline owner; absence is auto-scaffolded,
  not escalated. Acts on its assessment; never returns a report where action
  would suffice. Never returns red without a structured diagnosis.
  Bounded self-heal: auto-fix lint/format, regenerate lockfiles, retry
  flakes. Escalates only genuine algorithm/logic failures.
  Use when: "run ci", "check ci", "fix ci", "audit ci", "is ci passing",
  "run the gates", "dagger check", "why is ci failing", "strengthen ci",
  "tighten ci", "ci is red", "gates failing".
  Trigger: /ci, /gates.
argument-hint: "[--audit-only|--run-only]"
---

# /ci (canary)

Confidence in correctness for the `canary-obs` monorepo. CI is load-bearing:
green on canary means the core service, `canary_sdk/`, and `clients/typescript/`
all pass the same Dagger module that runs on Fly's hosted CI. Weak green is
a lie, so this skill **audits first, then runs**.

Stops at green. Does not review code semantics (→ `/code-review`), does not
address review comments or land (→ `/settle`), does not ship (→ `/deploy`).

## The Gate

There is one gate on canary, cited verbatim:

- `./bin/validate` — default. Delegates to `./bin/dagger check` (deterministic
  Dagger lanes + git-history secrets scan). Use this when you want "did CI
  pass locally."
- `./bin/validate --fast` — `dagger call fast`. Pre-commit set: format,
  compile-as-errors, entrypoint/DR shell tests for core and `canary_sdk/`,
  `typecheck` for `clients/typescript/`, `.codex/agents/*.toml` role
  validation. Wired into `.githooks/pre-commit`.
- `./bin/validate --strict` — `dagger call strict`. Everything in the
  deterministic lane **plus** live advisory scans (`mix deps.audit` for core
  and SDK, `npm audit --audit-level high` for the TS SDK) **plus** full
  `.codex/agents/*.toml` role validation. Wired into `.githooks/pre-push`.
- `./bin/validate --advisories` — `dagger call advisories` only. Use when
  triaging a dependency CVE in isolation.

`bin/dagger` refuses local drift: it parses `engineVersion` from
`dagger.json` (currently `v0.20.5`) and errors out if the installed
`dagger` CLI version does not match. It also selects a Docker transport on
macOS (`direct` when the local Docker client works, `colima-ssh` when
Colima is the only backend) so the same invocation works on operators'
laptops and Linux CI runners. Do not invent a bypass — fix the installed
version or the Docker runtime.

Hosted CI (`.github/workflows/ci.yml`) is an **immutable control plane**,
not a parallel pipeline. It uses `pull_request_target`, checks out the
trusted base snapshot to `.ci/trusted/` and the candidate diff to
`.ci/candidate/`, reads `engineVersion` from `.ci/trusted/dagger.json`,
and runs:

```bash
dagger call strict --source=../candidate
```

from the trusted working directory. A PR cannot weaken required checks by
editing `.github/workflows/ci.yml` or `dagger/` in the candidate diff —
only a merged change to `master` takes effect on subsequent PRs. The
authoritative runbook is `docs/ci-control-plane.md`; read it before
touching the workflow, the Dagger module, or the pin-by-commit actions.

`.github/workflows/deploy.yml` is downstream: it only fires
`flyctl deploy --remote-only` after `CI` reports success on `master`. If
CI is red, deploy cannot race it.

## Modes

- Default: audit → run. Full pass.
- `--audit-only`: produce audit report and gap proposals; do not run gates.
- `--run-only`: skip audit, just drive `./bin/validate` green.

## Stance

1. **Audit before run.** A weak pipeline passing is worse than a strong
   one failing. Inventory what `dagger call strict` actually covers before
   trusting green.
2. **`./bin/validate` is the only sanctioned gate.** Never substitute a
   raw `mix test`, `mix credo`, `npm run test`, or a standalone
   `dagger call` except for surgical bisection — and only after the full
   gate has already failed in a reproducible way. The user-ratified pattern
   on this repo is "trust `./bin/validate --strict` over raw mix
   composition." Raw shells bypass the hermetic-container contract that
   makes hosted CI reproducible locally.
3. **Act, do not propose.** Mechanical strengthenings (adding a missing
   gate to `dagger/src/index.ts`, raising a threshold when the code
   already passes a higher bar, pinning an action by commit in
   `.github/workflows/ci.yml`, wiring a new bin script into the
   `rootFast`/`rootQuality` lanes) are applied directly. Only escalate
   when the change would encode a product decision the code alone cannot
   resolve.
4. **Fix-until-green on self-healable failures.** Don't report red and
   exit. Either fix or produce a precise diagnosis.
5. **No quality lowering, ever.** Coverage floors (**81%** for core,
   **90%** for `canary_sdk/`), `mix compile --warnings-as-errors`,
   `mix credo --strict`, `mix sobelow --threshold medium`, `mix dialyzer`,
   `npm audit --audit-level high`, `--strict` on `dagger.json` pin — all
   load-bearing walls. Raise is fine; lower is forbidden.
6. **Bounded self-heal.** See `references/self-heal.md` for fix-vs-escalate.
   Algorithm and logic failures escalate.

## Process

### Phase 1 — Audit (skip if `--run-only`)

Full rubric in `references/audit.md`. Inventory in parallel. Pipeline
presence is not a gap here — `dagger.json` is canonical — but the
immutable control plane and per-lane coverage are both load-bearing.

- **Dagger entrypoint parity.** `dagger functions` (run via `./bin/dagger
  functions`) must list `fast`, `check`, `strict`, `advisories`. If a
  new lane has landed without being surfaced, that's a gap.
- **Hosted CI thinness.** `.github/workflows/ci.yml` must contain only:
  dual checkout to `.ci/trusted/` + `.ci/candidate/`, engine-version read
  from trusted `dagger.json`, and one `dagger call strict --source=../candidate`.
  Inline `mix ...`, `npm ...`, or raw bash beyond that is a finding —
  pipeline has leaked out of the Dagger module.
- **Control-plane pinning.** `actions/checkout` and
  `dagger/dagger-for-github` are pinned by SHA (`@<sha>` not `@v4`). Any
  unpinned action in this workflow is a HIGH finding per
  `docs/ci-control-plane.md`.
- **Gate coverage per package:**
  - Core (`rootQualityContainer` in `dagger/src/index.ts`): `mix compile
    --warnings-as-errors`, `mix format --check-formatted`, `mix credo
    --strict`, `mix sobelow --config --exit --threshold medium`,
    `mix test --cover` (floor **81%** from `mix.exs` `test_coverage:
    [summary: [threshold: 81]]`), `mix dialyzer`,
    `test/bin/entrypoint_test.sh`, `test/bin/dr_test.sh`.
  - `canary_sdk/` (`sdkQualityContainer`): `mix compile
    --warnings-as-errors`, `mix format --check-formatted`, `mix test
    --cover` (floor **90%** from `canary_sdk/mix.exs`).
  - `clients/typescript/` (`typescriptQualityContainer`):
    `npm run typecheck`, `npm run test:ci`, `npm run build`.
  - Contracts: `openapiContractContainer` (Redocly lint of
    `priv/openapi/openapi.json`), `apiContractsContainer`
    (`mix test --only contract`), `ciContractContainer`
    (`dagger/scripts/ci_contract_validation.py`).
  - Secrets: `gitleaks dir .` in `check`, `gitleaks git .` on `strict`
    (full history scan).
- **Live advisories.** `strict` composes `rootAdvisoryContainer`
  (`mix deps.audit`), `sdkAdvisoryContainer`, and
  `typescriptAdvisoryContainer` (`npm audit --audit-level high`). These
  are flaky by design — an upstream CVE advisory can flip strict red
  without any local change. Treat as signal, not noise.
- **Pre-commit / pre-push hooks.** `.githooks/pre-commit` must `exec
  bin/validate --fast`. `.githooks/pre-push` must `exec bin/validate
  --strict`. Anything else means local-CI contract has drifted.
- **Toolchain pins.** `.tool-versions` must pin Erlang 27.3.x, Elixir
  1.17.x-otp-27, Node 22.22.x to match the `hexpm/elixir` and `node`
  images referenced by digest in `dagger/src/index.ts`. Drift between
  `.tool-versions` and the pinned container digest is a finding — local
  runs will diverge from hosted CI.

Emit a structured audit report:

```markdown
## CI Audit (canary)
| Concern                   | Status | Severity | Fix                                        |
|---------------------------|--------|----------|--------------------------------------------|
| dagger functions surface  | ok     | -        | -                                          |
| ci.yml pinned-by-SHA      | ok     | -        | -                                          |
| core coverage (81%)       | ok     | -        | -                                          |
| sdk coverage (90%)        | ok     | -        | -                                          |
| ts audit level            | gap    | med      | Raise to `--audit-level moderate`          |
| history gitleaks on check | gap    | low      | Promote `secretsHistory` into `check` lane |
```

For each gap, apply the remediation directly — usually an edit in
`dagger/src/index.ts` and a `./bin/validate --strict` verification pass.

### Phase 2 — Run (skip if `--audit-only`)

1. Run the gate end-to-end: `./bin/validate` by default, or
   `./bin/validate --strict` when the run is a pre-merge verification or a
   pre-push equivalent. Do not shell out to `mix test` alone.
2. Capture per-lane output (which Dagger function, which `withExec` step,
   pass/fail, excerpt). The Dagger module makes this easy: failures name
   the container and the command.
3. If green → emit report, exit 0.
4. If red → classify each failure per `references/self-heal.md`:
   - **Self-healable** (`mix format` drift, `mix credo` style violation
     with a one-line fix, stale `mix.lock` / `package-lock.json`,
     transient `npm ci` network flake, a `.githooks/pre-commit` failure
     on newly generated code): dispatch a focused builder subagent,
     commit with a `fix(ci):` scope (this repo's convention — see
     recent history: `fix(ci): make github control plane immutable`,
     `fix(ci): harden local docker probe handling`), re-run the failing
     lane, not the full gate.
   - **Escalatable** (a failing `mix test --only contract` assertion, a
     Dialyzer error in a hand-written spec, a coverage drop below
     **81%** / **90%**, a failed `apiContractsContainer` run, a
     `sobelow` finding of medium severity): stop. Emit structured
     diagnosis (file:line, lane, excerpt, candidate cause). Exit
     non-zero. Coverage drops and contract-test failures are never
     self-heal candidates on this repo — they are product decisions.
5. Bounded retries: cap self-heal at **3 per lane**. If `mix format`
   keeps re-drifting, the file is generated or there's a hook
   mis-config — escalate.

### Phase 3 — Verify

Final pass of `./bin/validate` (or `./bin/validate --strict` if the run
was on the pre-push path). Green or bust. If Phase 1 raised a threshold
or added a lane, the full gate must pass under the new bar before
returning clean.

## Known Failure Patterns (prior fixes)

These have happened on master; expect relatives. Check `git log
--grep='^fix(ci):'` for canonical fixes.

- **Local Docker probe drift on macOS.** `bin/dagger` cannot reach the
  Docker socket. Fix landed in `bin/lib/docker_probe.sh` plus the
  `direct` / `colima-ssh` transport auto-selection. Commit scope:
  `fix(ci): harden local docker probe handling`. If you see
  `"Unknown Docker runtime backend"` or a mysterious Dagger
  connection error on macOS, re-read `bin/dagger` — the bug is almost
  always that Colima is up but the socket isn't discoverable.
- **Colima macOS SSH routing.** The `run_with_colima` shim in `bin/dagger`
  writes a temporary `docker` wrapper that execs over `ssh -F <colima
  ssh config>`. If Colima's SSH config path moves, this breaks. Fix
  lives in the `colima_ssh_config_path` helper in `bin/lib/docker_probe.sh`.
- **Dagger strict contract drift.** `dagger/scripts/ci_contract_validation.py`
  enforces that every Dagger `@func()` is properly exposed and that
  ignored-path lists in `@argument` decorators stay in sync. When this
  fails, the message names the drifting function — re-run
  `dagger/scripts/sync_source_arguments.py` then `./bin/validate
  --strict`. Commit scope example: `fix(ci): ...` touching
  `dagger/src/index.ts`.
- **Immutable CI control plane regression.** Any PR that introduces
  inline bash into `.github/workflows/ci.yml` or stops using
  `pull_request_target` is a regression of the model in
  `docs/ci-control-plane.md`. The canonical fix is commit
  `fix(ci): make github control plane immutable` — restore the
  `.ci/trusted/` + `.ci/candidate/` dual-checkout pattern.

## What /ci Does NOT Do

- Review code semantics → `/code-review`
- Shape tickets or write specs → `/shape`
- Address review comments or land the branch → `/settle`
- Deploy → `/deploy` (hosted `deploy.yml` fires on green master anyway)
- Monitor the deploy → `/monitor`
- QA against a running app → `/qa`
- Touch `Canary.Repo`, `Canary.Health.StateMachine`, or any hot module
  in `lib/canary/...` to "make a test pass" — coverage and contract
  failures are product decisions and escalate.
- Lower any threshold. Specifically: never edit `mix.exs`
  `test_coverage: [summary: [threshold: 81]]` down, never edit
  `canary_sdk/mix.exs` threshold down from `90`, never drop
  `--warnings-as-errors`, never drop `--strict` on credo, never raise
  `--threshold` on sobelow from `medium`, never drop `dialyzer`.

## Anti-Patterns

- **Running `mix test` directly** because "the full gate is slow." If
  the full gate is too slow, raise a backlog item in `backlog.d/` — do
  not bypass. The repo's cache-volume design in
  `dagger/src/index.ts::elixirContainer` makes warm runs fast.
- **Running `dagger call <lane>` without `./bin/dagger`.** The wrapper
  enforces the pinned engine version. A raw `dagger` call from PATH
  can silently run a different engine and produce a different result
  from hosted CI.
- **Editing `.github/workflows/ci.yml` to add an inline `mix` step.**
  This is a control-plane violation. Fold the gate into
  `dagger/src/index.ts`, expose it as a `@func()`, compose it into
  `strict()` or `deterministic()`, and let hosted CI pick it up
  automatically.
- **Auto-fixing a failing contract test by editing
  `priv/openapi/openapi.json` or `test/canary_web/contract/*`** — the
  OpenAPI contract is a load-bearing agent interface
  (`info.x-agent-guide` embedded in the spec). Escalate.
- **Auto-fixing a coverage failure by adding trivial tests** to hit
  numbers. Coverage drops name a gap in actual test intent; escalate.
- **Suppressing a Dialyzer warning** with a `@dialyzer` attribute or a
  `no_match`/`no_return` ignore. Escalate — it's a type/contract issue.
- **Declaring "green"** while `./bin/validate --strict` is still
  executing advisories. Wait for exit.
- **Running `flyctl deploy` directly** to "unstick" a red CI. Deploy
  fires only from `.github/workflows/deploy.yml` on green master —
  bypassing it bypasses the control plane.

## Output

```markdown
## /ci Report
Audit: 1 gap found (ts audit-level at `high`, bump to `moderate`).
  → Strengthened: `typescriptAdvisoryContainer` now runs
    `npm audit --audit-level moderate`.
  → Deferred: none.
Run: `./bin/validate --strict` — 13 lanes, 1 self-heal (mix format
     drift in `lib/canary/incidents.ex`, committed as
     `fix(ci): mix format drift in incidents.ex`), 0 escalations.
Final: green. Total 7m41s warm.
```

On failure:

```markdown
## /ci Report — RED
Lane: rootQuality → mix test --cover
Failure: test/canary/incident_correlation_test.exs:142
  coverage 80.4% < threshold 81%
Classification: threshold violation (coverage regression)
Action: escalated — decide whether to add targeted tests for the
  new `lib/canary/incident_correlation.ex` branches or roll back.
  Gate will not self-heal.
```
