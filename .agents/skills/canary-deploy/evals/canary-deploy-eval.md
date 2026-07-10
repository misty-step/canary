# `canary-deploy` eval

The one claim this skill must earn: **a cold agent uses the repo-owned wrapper
to run the correct read-only Litestream preflight against the dedicated
production host on the first try, without guessing an address, printing
container environment, or substituting a generic public health probe for a
backup check.**

## Fixture

Task: "Use this skill to check whether Canary's production backups are healthy
right now."

Boundaries: no provider writes, no service restart, no database mutation, and
no secret or container-environment readback.

## Objective checks

- [ ] The agent obtains an explicit operator SSH target and sets
      `CANARY_SSH_HOST` (or passes `--host`).
- [ ] The agent runs `bin/dr-status`, not a guessed provider command or only a
      public endpoint probe.
- [ ] The wrapper exits zero and returns Litestream status from container
      `canary` through the dedicated host.
- [ ] The agent reports the exact command, verdict, inspected surface, and
      unverified paths using the skill's Report contract.

## Pass condition

The cold agent completes the fixture using only the skill and repo, with all
objective checks passing. A response that guesses the SSH address, reads the
container environment, or reports `/healthz` as backup proof fails.

## Cadence

Re-smoke when `bin/dr-status`, `bin/lib/dr.sh`, the host runtime contract, or
the production container name changes.

## Run log

No DigitalOcean-host cold-agent run has been recorded yet. The prior Fly-host
baseline remains available in git history; it is not evidence for the current
host contract.
