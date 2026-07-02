# Triage GitHub issue #129 against the 048/062 backlog

Priority: P3 · Status: ready · Estimate: S

## Goal
GitHub issue #129 ("[P1] Tighten the external responder contract with
service-scoped context and annotation semantics", open since 2026-04-16, no
comments) is resolved into exactly one of: closed as superseded, or explicitly
linked as tracked-by an open `backlog.d/` ticket — so it stops appearing as a
second, un-cross-referenced backlog surface.

## Oracle
- [ ] Issue #129's stated goal (tighten the responder contract: service-scoped
      context, documented annotation-action vocabulary, no Canary-owned
      remediation runtime) is compared line-by-line against
      `backlog.d/048-responder-rich-context-safety-gate.md` and
      `backlog.d/062-agent-loop-write-surface.md`.
- [ ] If fully covered: issue is closed with a comment naming which ticket(s)
      supersede it and why (cite the specific Oracle bullets that match).
- [ ] If partially covered: issue stays open but gets a comment stating exactly
      what gap remains uncovered by 048/062, and that gap gets its own new
      backlog line item (or an addition to an existing ticket's Notes) rather
      than being silently dropped.
- [ ] No backlog `.md` file is deleted or silently merged — any consolidation
      is proposed in ticket text, per house grooming rules.

## Notes
Issue #129 predates both 048 (filed 2026-06-20) and 062 (filed 2026-07-01) and
reads as an earlier draft of the same problem: annotations that are "flexible"
but not a documented product contract, and a responder that fetches the global
`/api/v1/report` and filters client-side rather than getting service-scoped
context. That's close to word-for-word what 048/062 already scope. This is a
read-and-decide task, not a design task — the actual scope decision was
already made when 048/062 were filed; this ticket just closes the loop on the
older tracking surface.

**Why:** an open P1 GitHub issue that nobody is actively working, sitting
alongside two backlog tickets covering the same ground, is exactly the kind of
drift a groom pass exists to catch — cheap to fix, and it stops a future
contributor from treating #129 as separately-actionable scope.
