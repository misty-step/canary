# Canary -> Bitterblossom Triage Contract

Status: scoped contract for the Factory Canary lane on 2026-07-01. This
supports Bitterblossom backlog `080` (report-only Canary triage) and `081`
(authority ladder) without moving remediation authority into Canary.

## Boundary

Canary remains the observability substrate. It owns signal ingest, incident
correlation, timeline replay, signed webhook delivery, remediation claims, and
annotations. Bitterblossom owns the responder workload: materializing repo and
infra context, dispatching an agent, producing `REPORT.json`, and later staging
authority through its own policy gates.

The webhook is a wake-up hint, not the context source of truth. The responder
must query Canary before acting.

## Event To Trigger Flow

1. Canary records or updates a durable subject: `incident`, `error_group`,
   `target`, or `monitor`.
2. Canary sends the existing signed webhook payload with stable
   `X-Delivery-Id` and timestamped HMAC headers.
3. Bitterblossom verifies the signature and timestamp, dedupes by delivery id,
   and writes a run-ledger row before returning success to the webhook.
4. The `canary-triage` workload replays Canary state through MCP or CLI:
   `canary_incidents`, `canary_timeline`, `canary_errors`,
   `canary_services`, `canary_targets`, `canary_monitors`, and
   `canary_claims_list`.
5. The responder creates or observes a remediation claim before investigation:
   `canary_claim_create` with an idempotency key derived from the BB run id and
   Canary subject.
6. The report-only agent investigates outside Canary and writes `REPORT.json`.
7. The workload writes back an annotation/evidence link. At report-only
   authority, claim changes are limited to releasing Bitterblossom's own claim
   as a handoff marker; verification, incident resolution, branch/PR work, and
   deploys require a higher authority level.

## Minimum Trigger Payload

The webhook payload must be enough to route and replay, not enough to replace
replay. The shape below is the **actual live emitter output** as of
2026-07-02, pinned by a conformance test in
`crates/canary-store/src/incidents.rs`. A coordinated rename to a
`subject`+`schema_version:1` form is a FUTURE migration requiring lockstep
Bitterblossom changes (its task.toml filters on `/incident/service`).

```json
{
  "schema_version": "canary.incident_event.v1",
  "event": "incident.opened",
  "tenant_id": "",
  "project_id": "",
  "subject": {
    "type": "incident",
    "id": "INC-example",
    "service": "canary"
  },
  "signal": {
    "kind": "error_group",
    "fingerprint": "ERR-example",
    "severity": "warning",
    "observed_at": "2026-07-01T00:00:00Z"
  },
  "replay": {
    "timeline_url": "/api/v1/timeline?service=canary&window=1h",
    "report_url": "/api/v1/report?window=1h",
    "incident_url": "/api/v1/incidents/INC-example"
  },
  "incident": {
    "id": "INC-example",
    "service": "canary",
    "state": "investigating",
    "severity": "warning",
    "title": null,
    "opened_at": "2026-07-01T00:00:00Z",
    "resolved_at": null,
    "signals": [
      {
        "signal_type": "error_group",
        "signal_ref": "ERR-example",
        "attached_at": "2026-07-01T00:00:00Z",
        "resolved_at": null
      }
    ]
  },
  "timestamp": "2026-07-01T00:00:00Z"
}
```

Note: the `subject.environment` field referenced in earlier drafts of this
contract is not present in the live emitter today. Bitterblossom should
treat its absence as a report-only setup gap and query the missing value by
subject id.

If the current generic webhook payload lacks one of those fields, the
Bitterblossom workload should treat that as a report-only setup gap and query
the missing value by subject id. Canary should not add repo mutation commands,
branch names, PR policy, or deployment instructions to the event.

## Service To Repo Mapping

Canary should carry only stable service and project identifiers. The first
Bitterblossom implementation can use its `canary-services.toml` mapping:

- `service` or `project/service` selects the target repository.
- `environment` scopes infrastructure probes and secret-free deploy context.
- unmapped services halt as `mapping_missing` and produce a report-only
  artifact rather than guessing.

Canary may later expose a read-only service metadata field, but it should not
become a repo mutation router.

## Authority Levels

The Canary payload does not grant mutation authority. Bitterblossom controls
the authority ladder:

| Level | Name | Allowed |
|---|---|---|
| 0 | observe | verify webhook, replay Canary, inspect repo/infra context |
| 1 | report_only | write `REPORT.json`, annotations, and claim evidence |
| 2 | branch_pr | create a branch/PR after report-only scorecard is green |
| 3 | guarded_land | merge only with CI, fresh review, Canary sanity, and policy gate |
| 4 | own_change_rollback | revert only the responder's own last known change after sanity failure |

Automatic promotion is forbidden. Canary evidence can make the next authority
level eligible; it cannot approve it.

## Report-Only `REPORT.json`

The first workload must produce:

```json
{
  "schema_version": 1,
  "canary_subject": {"type": "incident", "id": "INC-example"},
  "delivery_id": "WHK-delivery",
  "bb_run_id": "run-example",
  "service": "canary",
  "repo": "misty-step/canary",
  "summary": "what happened",
  "evidence": [
    {"source": "canary_timeline", "ref": "timeline cursor or URL"},
    {"source": "canary_errors", "ref": "ERR or group hash"}
  ],
  "hypotheses": [
    {"claim": "likely cause", "confidence": "low|medium|high", "why": "bounded rationale"}
  ],
  "suspected_files_or_services": [],
  "recommended_next_commands": [],
  "residual_uncertainty": []
}
```

The report is invalid if the run changed code, pushed a branch, deployed, or
resolved the incident.

## Verification

- Replay fixture Canary payload through Bitterblossom manual run:
  `bb run canary-triage --payload-file <fixture> --json`.
- Confirm the BB run ledger has one accepted run and no duplicate run for the
  same delivery id.
- Confirm `REPORT.json` links the Canary subject, delivery id, service, repo,
  evidence sources, hypotheses, and residual uncertainty.
- Confirm the target repo has no diff and no outward side effects except
  Canary claim/annotation evidence.
- Confirm Canary timeline can replay the claim and evidence write-back.

This contract should be revisited when Canary backlog `048` defines narrower
responder-write authority. Until then, report-only dogfood should use existing
read/admin credentials with explicit containment in Bitterblossom.
