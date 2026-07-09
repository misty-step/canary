# Agent Loop Write Surface: Incident Conformance Receipt

Date: 2026-07-04
Scope: `backlog.d/062` incident slice only.

## Outcome

A cold agent can complete the incident loop through the CLI and through MCP
against a local Canary server:

1. list and read an incident
2. create a collision-safe remediation claim
3. annotate the incident with evidence links and actor identity
4. release the claim through CLI or verify it through MCP
5. replay the writes from incident detail and the service timeline

Responder keys remain service-bound. A responder key for `loop-api` could read
and mutate the `loop-api` incident and received RFC 9457 `403 insufficient_scope`
when attempting to annotate a `loop-other` incident.

Raw admin and responder secrets were redacted from this receipt. Key ids are
included only where they appear in durable audit/timeline evidence.

## Local Driver

```bash
CANARY_DB_PATH=/tmp/canary-062-loop.db \
CANARY_DISCLOSE_BOOTSTRAP_KEY=false \
PORT=4712 \
cargo run -q -p canary-server
```

Seeded durable subject:

```text
endpoint: http://127.0.0.1:4712
service: loop-api
incident_id: INC-hwe7en7qbxr0
responder_key_id: KEY-82xpkc5lh7hx
```

## CLI Transcript

```bash
bin/canary --endpoint http://127.0.0.1:4712 --api-key "$RESPONDER_KEY" --json incidents list --open
bin/canary --endpoint http://127.0.0.1:4712 --api-key "$RESPONDER_KEY" --json incidents get INC-hwe7en7qbxr0
bin/canary --endpoint http://127.0.0.1:4712 --api-key "$RESPONDER_KEY" --json claims claim \
  --subject-type incident \
  --subject-id INC-hwe7en7qbxr0 \
  --owner codex-cli \
  --purpose "live incident loop proof" \
  --ttl-ms 900000 \
  --idempotency-key canary-062-cli-primary \
  --evidence-link https://example.com/canary/062/claim
```

Claim result:

```text
claim_id: CLM-2m20vkapkbda
state: claimed
owner: codex-cli
subject: incident/INC-hwe7en7qbxr0
```

Second claimant was refused by the existing remediation-claim primitive:

```text
POST /api/v1/claims returned 409 Conflict
code: claim_conflict
current_claim.owner: codex-cli
current_claim.state: claimed
```

Cross-service mutation was refused with the responder key bound to `loop-api`:

```text
POST /api/v1/annotations returned 403 Forbidden
code: insufficient_scope
bound_service: loop-api
requested_service: loop-other
```

The same CLI identity then wrote evidence and released the claim:

```bash
bin/canary --endpoint http://127.0.0.1:4712 --api-key "$RESPONDER_KEY" --json annotations create \
  --subject-type incident \
  --subject-id INC-hwe7en7qbxr0 \
  --agent codex-cli \
  --action fix-verified \
  --metadata claim_id=CLM-2m20vkapkbda \
  --metadata evidence=https://example.com/canary/062/cli-proof \
  --metadata note="cli loop wrote durable evidence"

bin/canary --endpoint http://127.0.0.1:4712 --api-key "$RESPONDER_KEY" --json claims release \
  CLM-2m20vkapkbda \
  --owner codex-cli
```

Readback from `incidents get` after release:

```text
annotations: 1
claims: 1
claim state: released
recent_timeline_events:
  remediation_claim.released
  annotation.added
  remediation_claim.created
  incident.opened
```

## MCP Transcript

The MCP server was driven over stdio with `CANARY_RESPONDER_KEY` and no raw HTTP
calls by the client. Tool calls:

```text
canary_incident_get incident_id=INC-hwe7en7qbxr0
canary_claim_create subject_type=incident subject_id=INC-hwe7en7qbxr0 owner=codex-mcp
canary_annotation_create subject_type=incident subject_id=INC-hwe7en7qbxr0 agent=codex-mcp action=mcp-fix-verified
canary_claim_transition state=verified owner=codex-mcp evidence_link=https://example.com/canary/062/mcp-proof
canary_incident_get incident_id=INC-hwe7en7qbxr0
```

MCP response summary:

```text
id=2 command=canary_incident_get incident=INC-hwe7en7qbxr0
id=3 command=canary_claim_create state=claimed incident=INC-hwe7en7qbxr0
id=4 command=canary_annotation_create action=mcp-fix-verified incident=INC-hwe7en7qbxr0
id=2 command=canary_claim_transition state=verified incident=INC-hwe7en7qbxr0
id=3 command=canary_incident_get incident=INC-hwe7en7qbxr0 timeline=5
```

## Durable Timeline Readback

`bin/canary timeline --service loop-api --window 1h --limit 20 --json` replayed
the actor-bearing writes:

```text
remediation_claim.updated  incident INC-hwe7en7qbxr0  codex-mcp set incident INC-hwe7en7qbxr0 to verified.
annotation.added           incident INC-hwe7en7qbxr0  codex-mcp annotated incident INC-hwe7en7qbxr0 with mcp-fix-verified.
remediation_claim.created  incident INC-hwe7en7qbxr0  codex-mcp claimed incident INC-hwe7en7qbxr0.
remediation_claim.released incident INC-hwe7en7qbxr0  codex-cli released incident INC-hwe7en7qbxr0.
annotation.added           incident INC-hwe7en7qbxr0  codex-cli annotated incident INC-hwe7en7qbxr0 with fix-verified.
remediation_claim.created  incident INC-hwe7en7qbxr0  codex-cli claimed incident INC-hwe7en7qbxr0.
incident.opened            incident INC-hwe7en7qbxr0  loop-api: incident opened
```

## Oracle Verdicts

- MCP-only incident loop: pass for incidents. MCP read, claim, annotation, and
  verified transition succeeded through declared tools.
- CLI loop: pass for incidents. JSON envelopes used the same HTTP authority
  model and RFC 9457 failures surfaced as CLI errors.
- Service-bound responder key: pass for incident loop. Same-service reads/writes
  succeeded; cross-service annotation failed with `403 insufficient_scope`.
- MCP manifest authority wording: pass. The manifest now includes
  `canary_incident_get`; existing claim and annotation write tools remain the
  mutation path.
- Rich-context minimization/read audit from `048`: out of scope for this PR and
  still open. The incident slice does not widen responder read authority.

## Residuals

- `048-responder-rich-context-safety-gate.md` remains the P0 redaction and
  read-audit gate for richer responder context.
- Monitor/check-specific write ergonomics are intentionally deferred until the
  incident loop and `048` safety boundary are reviewed.
- Browser or screenshot evidence attachment is not part of this incident slice;
  annotations accept durable text/link metadata.
