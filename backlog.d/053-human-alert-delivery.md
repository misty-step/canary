# Human-facing alert delivery for real consumers (decision + optional bridge)

Priority: P2 · Status: pending · Estimate: M

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
