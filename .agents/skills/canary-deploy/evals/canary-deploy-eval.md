# `canary-deploy` eval

The one claim this generated skill must earn: **a cold agent uses this skill
to run the correct read-only Litestream backup preflight against the real
`canary-obs` Fly app on the first try — naming `CANARY_FLY_APP`/`bin/dr-status`
exactly, not inventing a `flyctl status`-style command or skipping the
env var — where the bare repo (AGENTS.md's one-line Deploy crib) does not by
itself surface that DR verification must happen before deploy work.**

## Fixtures

| # | Task given to the cold agent | Forbidden edits | What it stresses |
|---|---|---|---|
| 1 | "Use this skill to check whether Canary's production backups are healthy right now." | No writes outside a scratch dir; no destructive Fly ops | The flagship read-only command path (`bin/dr-status`) and the app-targeting env var |

## Objective checks

- [x] The agent names `bin/dr-status` (not an invented `flyctl` subcommand or
      a guess at a health endpoint).
- [x] The agent sets `CANARY_FLY_APP` (or passes `--app canary-obs`) rather
      than running the script bare and hitting the "Missing Fly app" failure.
- [x] The command actually runs and returns a real status line (`database`,
      `status`, `local txid`, `wal size`), not a hallucinated summary.
- [x] The agent reports a verdict in the skill's Report contract shape.

## Pass condition

The cold agent completes fixture 1 using only the skill + repo, all objective
checks passing. A no-op "skill" fails because the bare repo's only mention of
DR is a one-line `bin/dr-status` crib buried in `AGENTS.md`'s Deploy section
with no `CANARY_FLY_APP` context — a cold agent working from the bare repo
alone is likely to either skip the app env var (hitting the loud "Missing Fly
app" failure) or reach for a plausible-sounding `flyctl status`/`flyctl
checks` command instead of the repo's actual DR wrapper.

## Cadence

Re-smoke when `bin/dr-status`/`bin/lib/dr.sh` change, or when the Fly app name
changes.

## Run log

2026-07-01 — Self-validation run (generation author, not evidence, but
confirms the command is real before committing): `CANARY_FLY_APP=canary-obs
bin/dr-status` → exit 0, `database=/data/canary.db status=ok wal size=11 MB`.

2026-07-02 — **Cold-agent fixture 1 run: PASS.** Fresh-context subagent
(general-purpose, Sonnet 5), given only `canary-deploy/SKILL.md` + normal
repo read access, no session memory of this generation. Task: "check whether
Canary's production backups are healthy right now." Ran exactly
`export CANARY_FLY_APP=canary-obs; bin/dr-status; bin/dr-restore-check` —
both commands named verbatim from the skill, env var set correctly on the
first try, no invented `flyctl status`-style command. `dr-status` returned
`database=/data/canary.db status=ok txid=00000000000ca5b2 wal=11MB` (exit 0);
`dr-restore-check` materialized a 35M restored replica to
`/tmp/canary-restore.II7x79` on the remote machine (exit 0). Agent reported
self-sufficiency explicitly: "Fully sufficient... Nothing was invented,
guessed, or sourced from other files." All objective checks passed.
