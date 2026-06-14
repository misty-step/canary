# Remediation Claims

Canary remediation claims are the durable ownership primitive agents use before
they start automated triage or repair work. An annotation is a loose note; a
claim is coordination state with conflict semantics.

Claims apply to these subject types:

- `incident`
- `error_group`
- `target`
- `monitor`

Claim states are bounded:

- `claimed`
- `investigating`
- `fix_proposed`
- `verified`
- `dismissed`
- `expired`
- `released`

Only `claimed`, `investigating`, and `fix_proposed` are active ownership states.
An active, unexpired claim blocks a second owner from claiming the same subject.
Conflicts return RFC 9457 Problem Details with a `current_claim` object that
contains the current owner, state, purpose, expiration, and evidence links.

Agents create claims with an idempotency key. Replaying the same key for the
same subject returns the existing claim instead of creating a duplicate. Claim
creation expires old active claims whose TTL has elapsed before conflict checks
run.

Transition and release requests include the claim owner. Canary rejects mutation
attempts from a different owner, even when the caller has the right project and
service authority.

The canonical routes are:

- `GET /api/v1/claims?subject_type=...&subject_id=...&limit=20&cursor=...`
- `POST /api/v1/claims`
- `GET /api/v1/claims/{id}`
- `POST /api/v1/claims/{id}/transition`
- `POST /api/v1/claims/{id}/release`

Mutations require an `admin` key. Reads require `read-only` or `admin`.
Service-bound read keys can only see subjects in their service authority;
service-bound admin keys are rejected for mutations.

Claim lifecycle writes durable timeline events:

- `remediation_claim.created`
- `remediation_claim.updated`
- `remediation_claim.expired`
- `remediation_claim.released`

Timeline rows are the durable source of truth. Webhooks mirror the same claim
lifecycle hints for downstream responders; downstream systems own repository
mutation, issue creation, and LLM triage.

Existing agent read surfaces expose bounded claim state:

- incident lists and incident detail include `current_claim`
- error-group query and report rows include `current_claim`
- annotation pages include `current_claim` for the annotated subject
- timeline can be filtered by the remediation claim event names

The CLI exposes the same flow:

```bash
bin/canary claims list --subject-type incident --subject-id INC-example --limit 20
bin/canary claims get CLM-example
bin/canary claims claim --subject-type incident --subject-id INC-example --owner codex --purpose triage --ttl-ms 900000 --idempotency-key run-123
bin/canary claims transition CLM-example --owner codex --state investigating
bin/canary claims release CLM-example --owner codex
```
