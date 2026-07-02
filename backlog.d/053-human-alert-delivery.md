# Human-facing alert delivery for real consumers (decision + optional bridge)

Priority: P2 · Status: decision resolved, notifier in progress (see Resolution) · Estimate: M

## Goal
Decide how human operators of a Canary-monitored service get woken up, and provide a reference path — WITHOUT violating Canary's "webhooks wake, they don't decide / no opinionated integrations in core" stance.

## Why now
Habitat-dogfooding surfaced this. Canary's only outbound channel is generic HMAC-signed webhooks — by design (see #030 non-goal "Add GitHub/Slack/PagerDuty or other opinionated integrations", #047 non-goal "human dashboard", and the "webhooks wake, they don't decide" principle). A real external consumer (Habitat) needs a human notification (Slack/email) when prod breaks, and today every consumer must build that bridge from scratch with no reference.

## Decision to make
1. **Keep core webhook-only** (consistent with current architecture) and ship a documented, OPTIONAL reference consumer bridge (webhook → Slack/email) that lives OUTSIDE canary core — so consumers don't reinvent it; OR
2. **Revisit** the no-opinionated-integrations stance now that external consumers exist (heavier; risks scope creep into core and contradicts #030/#047).

Recommendation: option 1 — preserve the boundary, lower the consumer's cost with a reference bridge + documented signed-webhook contract.

## Oracle (if option 1)
- [ ] A copy-pasteable webhook → {Slack | email} bridge example exists (small worker/function), kept out of canary core.
- [ ] The signed-webhook contract + signature verification is documented for consumers.

## Relationship to existing backlog
Tensions DELIBERATELY with the #030 / #047 non-goals — filed as a DECISION item, not a commitment to build human integrations into core. The consumer-side need is also tracked on the Habitat side (HA-007 / HA-052: wire Canary's webhook → a thin human-alert path). Resolve the decision before either side builds.

## Resolution (2026-07-02)

Session-13 (escalation/paging capability) forced the decision this item flagged as never having a forcing consumer. The design spec at `~/.factory-lanes/wave1/escalation-spec.md` resolved it as **option 1** exactly as recommended here: Canary stays webhook-only in core (`incident.escalated` / `incident.deescalated` added to `BUSINESS_EVENTS`, riding the existing signed-webhook/delivery-ledger/retry machinery — zero new transport code), and the actual email/text delivery lives in a **reference notifier built outside Canary core**, in the sibling `bastion` repo, as a separate lane. That notifier is what finally builds the bridge this item asked for — it was not yet merged as of this Canary-side PR.

Canary-side primitive shipped in this PR: `incidents.escalated_at` overlay (orthogonal to `incidents.state`, never a value of that enum), `POST /api/v1/incidents/{id}/escalate` + `.../deescalate` (idempotency-keyed, `responder-write`-scoped, auto-clears on resolution in the same transaction), CLI parity (`bin/canary incidents escalate/deescalate`), and the two new webhook event names. This satisfies the "documented signed-webhook contract" half of this item's oracle; the "copy-pasteable webhook → email/Slack bridge" half is the bastion notifier's job, tracked there, not duplicated here.

The oracle checklist above is left as-is — this item does not close until the reference bridge is live. Do not file a duplicate backlog item for the notifier; it is the thing this item already asked for.
