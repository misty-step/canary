# Archive shipped-but-unarchived backlog tickets (054, 055, 059)

Priority: P3 · Status: ready · Estimate: S

## Goal
`backlog.d/` stops listing tickets as open once their oracle is already satisfied
in `master`, so the active count reflects real remaining work.

## Oracle
- [ ] `backlog.d/059-rustsec-anyhow-bump.md` is moved to `backlog.d/_done/`, with a
      note citing commit `fc87c2a` ("fix(deps): bump anyhow to clear
      RUSTSEC-2026-0190 (#185)", which itself says "Closes #059") and confirming
      `Cargo.lock` pins `anyhow 1.0.103`.
- [ ] `backlog.d/055-principles-rust-cutover-refresh.md` is moved to
      `backlog.d/_done/`, with a note citing commit `30aaa58` ("docs(principles):
      refresh examples to Rust+SQLite era (#186)") and confirming
      `grep -niE "genserver|oban|ecto|\.(ingest|transition|compute_group_hash)/[0-9]" PRINCIPLES.md`
      returns no matches.
- [ ] `backlog.d/054-serving-model-self-hosted.md` is moved to `backlog.d/_done/`,
      with a note confirming `VISION.md` already carries a "Serving Model"
      section (added in today's groom commit `9639358`) stating the three-way
      self-hosted / managed-later / no-multi-tenant-by-default distinction, and
      that it reconciles with "What Canary Is Not" and the "Use Canary when..."
      comparison table without contradiction.
- [ ] No other backlog file is touched, renamed, or renumbered.

## Notes
Found during 2026-07-01 refill-lane runway verification: the resident lane
closed all three of these earlier tonight (anyhow bump, PRINCIPLES refresh,
and the groom pass's own VISION.md edit satisfy each ticket's oracle verbatim)
but never `git mv`'d the ticket file to `_done/`. Pure filing hygiene — no code
change, no design call. Read each ticket's `## Oracle` section first and
re-verify against live `master` before moving anything, in case work has moved
further between when this was filed and when it's picked up.

**Why:** an accurate open-ticket count is what makes runway verification (this
refill lane's whole job) possible; three phantom-open tickets were undercounting
the resident lane's real progress and could cause a future groom pass to
re-propose work that's already done.
