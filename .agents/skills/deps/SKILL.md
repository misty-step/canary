---
name: deps
description: |
  Analyze, test, and upgrade dependencies. One curated PR, not 47 version bumps.
  Reachability analysis, behavioral diffs, risk assessment. Package-manager agnostic.
  Use when: "upgrade deps", "dependency audit", "check for updates",
  "outdated packages", "security audit deps", "update dependencies",
  "vulnerable dependencies", "deps".
  Trigger: /deps.
argument-hint: "[audit|security|upgrade <pkg>|report] [--surface core|sdk|ts|dagger|toolchain]"
---

# /deps

Analyze, test, and upgrade dependencies across canary's four dependency
surfaces. One curated PR per surface (or per logical bump); never a
47-package avalanche.

**Target:** $ARGUMENTS

## Execution Stance

You are the executive orchestrator.
- Keep upgrade policy, risk acceptance, and final merge-readiness judgment
  on the lead model.
- Delegate per-surface analysis and bounded upgrade work to focused
  subagents, one surface at a time.
- Parallelize across disjoint surfaces where safe (core Hex vs TS SDK vs
  Dagger module are independent). Do not parallelize inside a single
  `mix.lock` or `package-lock.json`.

## Canary Dependency Surfaces

Canary is a monorepo. Every `/deps` run must name all four surfaces plus
the Dagger CLI pin. Treating canary as single-ecosystem (Hex-only or
npm-only) is a failure.

| # | Surface | Manifest | Lockfile | Run from |
|---|---------|----------|----------|----------|
| 1 | **Core Hex** (Phoenix + Ecto + Oban + Req/Finch) | `mix.exs` | `mix.lock` | repo root |
| 2 | **canary_sdk Hex** (Req + ExDoc + mix_audit) | `canary_sdk/mix.exs` | `canary_sdk/mix.lock` | `canary_sdk/` |
| 3 | **TS SDK npm** (tsup, vitest, typescript) | `clients/typescript/package.json` | `clients/typescript/package-lock.json` | `clients/typescript/` |
| 4 | **Dagger TS module** (typescript, @redocly/cli) | `dagger/package.json` | `dagger/package-lock.json` | `dagger/` |

Plus two transversal pins that are **load-bearing** and not discovered by
outdated commands:

- **Dagger CLI pin** — `dagger.json` `engineVersion` (currently `v0.20.5`).
  `bin/dagger` refuses to run if the installed CLI drifts from this
  value. Hosted CI runs the exact same pin via the immutable control
  plane (`docs/ci-control-plane.md`). Bumps are a coordinated ritual —
  see "Dagger CLI bumps" below.
- **Toolchain pins** — `.tool-versions`: Erlang `27.3.4.9`, Elixir
  `1.17.3-otp-27`, Node `22.22.0`. CI resolves the same `.tool-versions`.
  Bump only when a Hex or npm upgrade forces it, and update CI images
  in the same PR.

`--surface core|sdk|ts|dagger|toolchain` limits the run to one surface.
Default: scan all four surfaces sequentially; short-circuit to a per-
surface PR, one merge at a time.

## Routing

| Mode | Intent |
|------|--------|
| **audit** (default) | Full: discover outdated, analyze risk, upgrade, test via `./bin/validate --strict`, PR |
| **security** | Advisory-only: `./bin/validate --advisories` (Hex via `mix_audit` + `npm audit`) with reachability into `lib/canary/*` |
| **upgrade** [pkg] | Targeted: upgrade a specific package with full analysis on its surface |
| **report** | Analysis only, no upgrades — produce the report |

### Mode → Phase Matrix

| Mode | Phase 0 | Phase 1 | Phase 2 | Phase 3 | Phase 4 | Phase 5 |
|------|---------|---------|---------|---------|---------|--------|
| audit | ✓ | ✓ | ✓ | ✓ | ✓ | PR |
| security | ✓ | ✓ (advisories only) | ✓ | ✓ | ✓ | PR |
| upgrade [pkg] | ✓ | skip | ✓ | ✓ | ✓ | PR |
| report | skip | ✓ | ✓ | skip | skip | Report only |

## Workflow

Six phases, gated. Each phase must complete before the next begins. Do
not cross-mix surfaces inside a single phase run — one surface at a time.

### Phase 0: Baseline

Baseline is **not** `mix test` alone. It is the full strict gate:

```bash
./bin/validate --strict
```

This runs `dagger call strict` — deterministic Dagger lanes for core
(compile, format, credo, sobelow, coverage **81%**, dialyzer),
`canary_sdk/` (compile, format, coverage **90%**), TS SDK (typecheck,
coverage, build), git-history secrets scan, and live dependency
advisories. If it fails, **STOP**. Fix the baseline before touching any
manifest. You cannot attribute regressions to upgrades against a red
baseline.

Narrow test runs are for per-surface triage only, per user-ratified
pattern: `mix test test/canary/<area>/<area>_test.exs --trace --max-failures 3`.

Gate: `./bin/validate --strict` green locally.

### Phase 1: Discover

Run per-surface outdated + advisory commands. Capture structured output.

**Core Hex** (from repo root):

```bash
mix hex.outdated
mix deps.audit                         # via mix_audit
./bin/validate --advisories            # canonical live advisory scan
```

**canary_sdk Hex** (from `canary_sdk/`):

```bash
cd canary_sdk && mix hex.outdated && mix deps.audit
```

**TS SDK npm** (from `clients/typescript/`):

```bash
cd clients/typescript && npm outdated --json && npm audit --json
```

**Dagger TS module** (from `dagger/`):

```bash
cd dagger && npm outdated --json && npm audit --json
```

**Dagger CLI pin** — compare `jq -r .engineVersion dagger.json` against
latest release. Drift is intentional until bumped.

**Toolchain pins** — compare `.tool-versions` against upstream Erlang
release series and Elixir/OTP compatibility matrix. Node LTS track.

Categorize each outdated dep:

- **Patch** (1.2.3 → 1.2.4): Safe. Apply without analysis.
- **Minor** (1.2.3 → 1.3.0): Usually safe. Quick changelog scan.
- **Major** (1.2.3 → 2.0.0): Needs full analysis. Breaking changes likely.

Use the canonical live scan (`./bin/validate --advisories`) as the
source of truth for advisories, not a one-off `mix hex.audit` or
`npm audit` in isolation — the strict gate aggregates all four surfaces.

Gate: structured per-surface list with categorization + advisory list.

### Phase 2: Analyze

For each non-patch update AND all advisory-flagged deps, analyze three
concerns. Parallelize across packages, not within a single package.

**Changelog.** Read upstream changelog/release notes. Summarize breaking
changes, deprecations. Verdict: `migration_required: yes | no | unknown`.

**Reachability.** Trace imports to flagged functions. See
`references/reachability-analysis.md`. Canary-specific reachability
hints:

| Package | Surface | Reachability — hot modules |
|---------|---------|----------------------------|
| `req`, `finch` | core Hex | `lib/canary/webhooks/delivery.ex`, `lib/canary/health/checker.ex`. **Footgun**: `Req.request/1` cannot take both `:finch` and `:connect_options` — use `:receive_timeout`. Any Req major that changes option semantics hits both modules. |
| `ecto`, `ecto_sql`, `ecto_sqlite3` | core Hex | All `lib/canary/schemas/*.ex` + `priv/repo/migrations/*`. **Invariant**: `Canary.Repo` pool_size:1 (single writer). Any driver change that affects WAL/transaction semantics requires full DR smoke via `bin/dr-restore-check`. |
| `oban` | core Hex | `lib/canary/workers/*`. **Footgun**: Oban Lite SQLite tables are created by dedicated migration `priv/repo/migrations/20260314230000_create_oban_jobs.exs`, never in a GenServer. An Oban major that touches the Lite engine schema requires a new migration, not an edit to the existing one. |
| `phoenix`, `phoenix_ecto` | core Hex | `lib/canary_web/router.ex`, `lib/canary_web/problem_details.ex`. Router pipeline scopes (`:scope_ingest`/`:scope_read`/`:scope_admin`) must survive any Phoenix router macro changes. |
| `bandit` | core Hex | Endpoint under Phoenix. Fly.io binding is load-bearing: `runtime.exs` must keep `port:` in the `http:` keyword list (documented footgun). |
| `bcrypt_elixir`, `nanoid` | core Hex | API key hashing + `ERR-`/`INC-`/`WHK-` ID generation. Custom string PKs must be set on the struct, not cast — a nanoid major that changes alphabet must not break existing rows. |
| `telemetry_metrics*`, `telemetry_poller` | core Hex | Self-observability metrics — see `lib/canary_web/router.ex` Prometheus exporter. |
| `credo`, `dialyxir`, `sobelow`, `mix_audit` | core Hex (dev) | Quality gates. A bump that changes default rules can red-line `./bin/validate --strict`. Treat rule-set changes as code changes — never silence, always fix. |
| `req` | canary_sdk Hex | `canary_sdk/lib/canary_sdk/**` — Logger handler HTTP. Keep behavior identical to core `Req` usage. |
| `tsup`, `vitest`, `typescript`, `@vitest/coverage-v8` | TS SDK | `clients/typescript/src/**`, `test/**`. Coverage gate is enforced — do not silence. |
| `typescript` | dagger TS module | `dagger/src/**` — entire strict gate definition. A TS major that changes module resolution or emit semantics can invalidate the pipeline definition. Regression-test via `./bin/validate --strict` immediately. |
| `@redocly/cli` | dagger TS module | OpenAPI lint in the strict gate. Flags against `priv/openapi/` source, which embeds the agent contract (`info.x-agent-guide`). |

Verdict: `reachable | not reachable | unknown`.

**Behavioral.** Compare API surface before/after. Check install scripts,
network calls, permission changes. See `references/behavioral-diff.md`.
Verdict: `risk: critical | high | medium | low`.

Gate: all packages have verdicts for all three concerns. Any `unknown`
reachability on a critical/high advisory → investigate deeper or escalate.

### Phase 3: Upgrade

Create branch `deps/<surface>-YYYY-MM-DD` (e.g. `deps/core-2026-04-20`,
`deps/ts-sdk-2026-04-20`). One branch per surface — **never mix surfaces
in a single branch**. Makes revert, bisect, and review tractable.

Apply upgrades in risk order within the branch:

1. **Patches** — all at once, one commit (`chore(deps): bump patches on <surface>`)
2. **Advisory fixes** — one commit per fix (clean revert path)
3. **Minors** — grouped by surface, one commit per logical group
4. **Majors** — one commit per package (isolation for bisect)

Commit prefix convention per canary `git log`:

- `chore(deps): bump <pkg> <from> -> <to>` — routine bumps
- `build: bump dagger engineVersion to vX.Y.Z` — Dagger CLI bump (load-bearing)
- `build: bump .tool-versions` — toolchain pin
- `fix(deps): <advisory-id> in <pkg>` — advisory remediation

Per-surface upgrade commands:

| Surface | Targeted bump | Full update | Cleanup |
|---------|---------------|-------------|---------|
| Core Hex | `mix deps.update <pkg>` | `mix deps.update --all` (avoid) | `mix deps.clean --unused --unlock` |
| canary_sdk Hex | `cd canary_sdk && mix deps.update <pkg>` | — | `cd canary_sdk && mix deps.clean --unused --unlock` |
| TS SDK | `cd clients/typescript && npm update <pkg>` | — | `cd clients/typescript && npm prune` |
| Dagger module | `cd dagger && npm update <pkg>` | — | — |

#### Dagger CLI bumps (load-bearing ritual)

The Dagger CLI pin in `dagger.json` is enforced by `bin/dagger` —
drift fails every validate invocation, local and hosted. Bump protocol:

1. Edit `dagger.json` `engineVersion` to the new `vX.Y.Z`.
2. Install the matching Dagger CLI locally (`bin/dagger` will refuse to
   proceed if the installed CLI version differs from the file).
3. Run `./bin/validate --strict` to regression-test the full pipeline
   definition under the new engine. Module TS (`dagger/src/**`) compiles
   against the new SDK surface.
4. Confirm hosted CI will accept the bump — `.github/workflows/ci.yml`
   uses `pull_request_target` against a trusted snapshot; the bump must
   pass `dagger call strict --source=../candidate` from the immutable
   control plane before merge. See `docs/ci-control-plane.md`.
5. Commit: `build: bump dagger engineVersion to vX.Y.Z` (isolated).

#### Toolchain (`.tool-versions`) bumps

Bump Erlang/Elixir/Node only when a Hex or npm upgrade forces it, or
when a CVE in the runtime demands it. CI images resolve the same
`.tool-versions` — always verify the trusted CI image has the new
version before merging. A toolchain bump is always a dedicated commit:
`build: bump .tool-versions (erlang|elixir|nodejs)`.

If a major has no migration guide and significant API changes,
**escalate to human**. Don't guess at migration.

Gate: upgrades committed atomically, one surface per branch.

### Phase 4: Test

After each upgrade group:

1. **Core Hex / canary_sdk / TS SDK / Dagger module**: re-run the full
   strict gate — `./bin/validate --strict`. This is non-negotiable. Do
   **not** substitute `mix test` alone; coverage floors (**81%** core,
   **90%** `canary_sdk/`), format, credo, sobelow, dialyzer, typecheck,
   and live advisories all gate via strict.
2. **Hot-path sanity** for webhook/health-critical bumps (Req, Finch,
   Ecto, Oban): narrow test run `mix test test/canary/<area>/... --trace
   --max-failures 3` on `lib/canary/webhooks/**`, `lib/canary/health/**`,
   `lib/canary/workers/**`.
3. **Litestream-adjacent bumps** (Ecto, ecto_sqlite3, or anything that
   could affect SQLite WAL or backup restore): run DR smoke via
   `bin/dr-status` + `bin/dr-restore-check` per `docs/backup-restore-dr.md`.
   Litestream itself is pinned in `Dockerfile` / `bin/entrypoint.sh`,
   not in a Hex lockfile — a Litestream bump is a Dockerfile edit, not
   a `/deps` target, but its interaction with ecto_sqlite3 bumps is.
4. **Dagger CLI / `dagger/` module bumps**: in addition to `--strict`
   locally, wait for hosted CI — the immutable control plane is the
   source of truth. See `docs/ci-control-plane.md`.

If strict fails: bisect within the group, revert the offending package,
note it in the report as "upgrade blocked — strict gate fails at
<stage>". Do not loosen a quality gate to land an upgrade. **Red Line.**

Gate: `./bin/validate --strict` green on the branch tip, plus any
surface-specific smokes.

### Phase 5: Report

One PR per surface (or per logical bump). Structured body:

```markdown
## Dependency Upgrades — <surface>

### Summary
<N> packages upgraded, <M> advisory fixes, <K> blocked (with reasons).
Surface: core Hex | canary_sdk Hex | TS SDK npm | Dagger TS module | Dagger CLI pin | toolchain.

### Advisories
| ID | Package | Surface | Severity | Reachable? | Action |
|----|---------|---------|----------|------------|--------|
| GHSA-xxxx | req | core Hex | High | Yes — `lib/canary/webhooks/delivery.ex:L<n>` | Upgraded 0.5.8 → 0.5.9 |
| GHSA-yyyy | some-dev-dep | TS SDK | Medium | No — devDependencies only | Upgrade queued (non-blocking) |

### Upgrades
| Package | From | To | Type | Risk | Reachability | Changelog |
|---------|------|----|------|------|--------------|-----------|
| phoenix | 1.8.0 | 1.8.1 | Patch | Low | router + dashboard LV | No breaking changes |
| oban | 2.18.0 | 2.19.0 | Minor | Medium | `lib/canary/workers/*` | Oban Lite schema unchanged |

### Reachability Report
<which flagged functions are actually called; cite `lib/canary/...` paths>

### Behavioral Changes
<install scripts added/removed, network calls, permission changes, Dagger
engine behavior drift if CLI was bumped>

### Gate Evidence
- `./bin/validate --strict` — green at <sha>
- Coverage: core **81%** (floor), canary_sdk **90%** (floor)
- Narrow smokes: <list of `mix test ... --trace --max-failures 3` runs>
- DR smoke (if applicable): `bin/dr-status` + `bin/dr-restore-check` OK

### Risk Assessment
<Overall risk: low/medium/high. Rationale. Residual risks. Rollback plan —
Fly.io deploy via `flyctl deploy --app canary-obs --remote-only` is
reversible via `flyctl releases rollback`.>
```

For **report** mode: produce without creating a branch or PR.
For **security** mode: include only Advisories, Reachability, and Gate
Evidence sections.

**Merge gate.** `./bin/validate --strict` locally + hosted CI's
`dagger call strict --source=../candidate` via the immutable control
plane (`.github/workflows/ci.yml`). Squash-merge on land via
`gh pr merge --squash`.

## Gotchas

- **Treating canary as single-ecosystem.** Four surfaces + Dagger CLI
  pin + toolchain pins. Every scan must name all four. A core Hex bump
  is not a `/deps` run — it's a surface of one.
- **Running `mix hex.audit` in isolation.** The canonical scan is
  `./bin/validate --advisories`, which aggregates advisories across
  surfaces and feeds the same gate as hosted CI. Raw `mix hex.audit` or
  `npm audit` output alone is a fragment, not a verdict.
- **Skipping strict gate for "just a patch."** Coverage floors, credo,
  sobelow, dialyzer, typecheck — all gate on `--strict`. A patch bump
  that quietly drops coverage below **81%** / **90%** is a regression.
  **Red Line: never lower a gate to land an upgrade.**
- **Drifting the Dagger CLI locally.** `bin/dagger` refuses to run if
  the installed CLI version differs from `dagger.json` `engineVersion`.
  Intentional — do not work around it. Bump via the coordinated ritual
  (edit `dagger.json` → install CLI → `--strict` → hosted CI confirms).
- **Batch-upgrading across surfaces in one branch.** You cannot bisect
  a branch that mixes `mix.lock`, `canary_sdk/mix.lock`, and
  `clients/typescript/package-lock.json`. One surface per branch.
- **Upgrading Ecto or ecto_sqlite3 without DR smoke.** SQLite WAL
  semantics are load-bearing for Litestream backups. Run `bin/dr-status`
  + `bin/dr-restore-check` before merge; see `docs/backup-restore-dr.md`.
  Related footgun: `rm -f /data/canary.db` on a live Fly machine is a
  no-op (WAL keeps the file handle). Stop → rm → start.
- **Bumping Req/Finch without re-reading the footgun.** `Req.request/1`
  cannot take both `:finch` and `:connect_options`. Every delivery +
  health-check site uses `:receive_timeout`. A Req major that changes
  option semantics hits `lib/canary/webhooks/delivery.ex` and
  `lib/canary/health/checker.ex` first.
- **Bumping Oban without checking the Lite migration.** Oban Lite tables
  are created by dedicated migration `priv/repo/migrations/20260314230000_create_oban_jobs.exs`.
  An Oban major that touches Lite schema requires a new migration, never
  an edit in place — and never in a GenServer.
- **Bumping Phoenix/LiveView without re-exercising router scopes.**
  `:scope_ingest` / `:scope_read` / `:scope_admin` pipelines enforce API
  key scoping. Every Phoenix major must keep these pipelines intact and
  the OpenAPI agent contract (`priv/openapi/`, `info.x-agent-guide`)
  unchanged.
- **Skipping `mix deps.clean --unused --unlock` after a major bump.**
  Transitive deps linger in `mix.lock`. Lockfile diff is the truth.
- **Trusting changelogs as complete.** Changelogs omit things. For
  majors, read the diff. The behavioral analyst catches what changelogs
  miss.
- **Upgrading without a lockfile.** Every canary surface has one —
  `mix.lock`, `canary_sdk/mix.lock`, `clients/typescript/package-lock.json`,
  `dagger/package-lock.json`. If one is missing, generate it before
  starting.
