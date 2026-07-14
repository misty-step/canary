# Design spike: fleet conflict primitive (leases over shared capacity)

Status: spike (canary-932 child 6). Feeds canary-063 (triage-contract
hardening: dispatch budgets, duplicate-responder control). Not an
implementation commitment.

## The failure class, observed live

Remediation claims coordinate *exclusive ownership of a subject*: one active
claim per incident/error_group/target/monitor, enforced by a partial unique
index. They say who is working *what*. They say nothing about how much of a
*shared, finite capacity* the fleet is consuming while it works.

The gap is not hypothetical. On 2026-07-14, incident `INC-mcklwuvfrlfr`
(service `sploot`, high severity, ~1,676 correlated signals) was triaged
end-to-end through a Canary claim (`CLM-albomrljutse`). Root cause: an
embedding cron re-admitting work against a rate-limited provider for 3.5
days with no backoff — 4,484 `EmbeddingAdmissionError` occurrences. One
process, one provider quota, no coordination. Multiply by a fleet of
responder agents woken by webhooks and the same shape appears *between*
agents: N agents independently triaging N incidents can all hit the same
LLM/API/provider quota at once, starve each other, and retry-storm — the
documented "agents colliding over shared rate limits" class.

Claims cannot express this because the contended thing is not a subject:
it has capacity greater than one, it is consumed in units, and it recovers
over time.

## What Canary should and should not own

Canary is the coordination substrate: durable, replayable records that
agents read, claim, annotate, and release (VISION, responder boundary).
Canary is not an execution engine and cannot *enforce* a rate limit on
traffic that never passes through it. Any primitive here is therefore
**advisory coordination**: agents that cooperate get collision-free
scheduling; agents that bypass it are no worse off than today. That is the
same trust model claims already use (owner is a string, honored by
convention, audited by events) — proven sufficient in the child-5 dogfood,
including crash-recovery by a successor process from the durable record.

A second boundary rule from the footgun list: Canary's own request-path
rate limiting is process-local; nothing here changes that. Leases are a
*product surface for the fleet*, not an internal limiter.

## Options considered

**A. Generalize claims to N-holder leases (recommended).** A `lease` is a
claim whose subject is a named *resource* with `capacity > 1`. Acquire
succeeds while active leases on the resource are fewer than capacity;
otherwise 409 with the current holders (mirror of the claim-conflict body).
TTL, heartbeat/renew, release, expiry sweep, lifecycle events, and fleet
visibility (`GET /api/v1/leases/active`) all reuse the claim machinery and
idioms wholesale — one concept extended, not a second coordination system.
Wire shape (sketch, not contract):

- `POST /api/v1/resources` (admin): `{name, capacity, window_hint_ms?}` —
  a registry row, no service names hardcoded, configured at runtime like
  targets/monitors/webhooks.
- `POST /api/v1/leases`: `{resource, owner, purpose, ttl_ms,
  idempotency_key}` → 201, or 409 `{current_holders: [...]}`.
- `POST /api/v1/leases/{id}/renew`, `POST /api/v1/leases/{id}/release`.
- `GET /api/v1/leases/active?resource=` — who holds what, newest first
  (same envelope family as `claims/active`).
- Events: `lease.acquired`, `lease.renewed`, `lease.expired`,
  `lease.released` — timeline-replayable like claim events.
- DB enforcement of capacity: counted check inside the single-writer
  transaction (the partial-unique-index trick only enforces capacity = 1;
  capacity = N needs a `SELECT count(*) ... FOR` check under the writer
  lock, which the single-writer model gives us for free).

**B. Token-bucket budget ledger in Canary.** Real consumption accounting
(`spend(n)` against a replenishing budget). Rejected for v1: it turns
Canary into a metrics/accounting store on the hot path of *other services'*
traffic (violates the bounded-payload posture the Estate contract just
re-affirmed), demands clock-driven replenishment machinery, and its honesty
still depends on cooperating callers — all of the trust model, far more
surface. A lease with a short TTL approximates a budget slot well enough
for the observed failure class.

**C. Advisory signals only (events + etiquette).** Emit
`resource.saturated` events and let responders back off. Cheapest, but it
has no conflict primitive at all: two agents reading the same event still
race. Insufficient alone; the *event* half is subsumed by A's lifecycle
events.

**D. Push entirely to consumers.** Status quo. The sploot storm is the
evidence against it.

## Why A is the Ousterhout choice

It deepens an existing module instead of adding a sibling: the claim
subsystem already owns durable TTL'd ownership records with conflict
semantics, expiry sweeps, redaction-reviewed read surfaces, CLI/MCP parity
and a contract-parity guard. Capacity-N acquire is one new axis on that
machinery. The alternative — a bespoke limiter service — would duplicate
every one of those surfaces and still share the advisory trust model.
Interface stays small (acquire/renew/release/list); complexity (capacity
counting, expiry, events) hides behind it.

## Open questions for the implementation card

1. Table shape: extend `remediation_claims` with nullable
   `resource`/`capacity` (one table, one sweep) vs. a `leases` table
   (cleaner types, second sweep). Lean: separate table, shared helper
   idioms — claims' subject CHECK constraints do not want loosening.
2. Does acquire *block-with-position* (return a queue slot) or fail-fast
   409? Lean fail-fast + `retry_after_hint_ms` derived from soonest lease
   expiry — no queue state to strand.
3. Renewal semantics vs. the `oban_jobs` worker-lease precedent already in
   the codebase (webhook delivery claims) — reuse its clock discipline.
4. Scope enforcement: leases are cross-service by nature (a shared provider
   quota is not service-bound), so read visibility follows the
   `claims/active` model (bound keys see their service's leases; unbound
   see all) but *acquire* likely needs `responder-write` without service
   pinning to the resource — needs the canary-936 ADR treatment.
5. Capacity changes while leases are outstanding (shrink below current
   holder count): document as "no revocation; drains by TTL".

## Relationship to canary-063

canary-063 wants durable webhook cooldown, dispatch budgets, and
claim-gated delivery. Leases give it the vocabulary: a dispatch budget is a
resource with capacity N; claim-gated delivery is "hold a lease on
`responder-dispatch` before acting on a webhook". This spike deliberately
stops at the primitive; 063 owns wiring it into delivery policy.
