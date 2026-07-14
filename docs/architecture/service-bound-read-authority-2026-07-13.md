# Service-Bound Read Authority

Status: accepted  
Date: 2026-07-13  
Decision: canary-936

## Context

Canary previously treated `api_keys.service IS NULL` as project-wide read
authority. That made the most powerful read credential the default result of
omitting a field, and the stored row could not distinguish an intentional
fleet reader from a legacy or malformed key. Rich incident and error context
then amplified the blast radius of one leaked read key.

Canary is self-hosted and infrastructure-agnostic. This decision therefore
belongs to the product authorization contract, not Misty Step deployment
configuration: every operator needs the same safe default regardless of where
Canary runs.

## Decision

New `read-only` keys must choose exactly one authority shape:

- `service=<name>` grants reads for one service; this is the default path.
- `allow_unbound=true` explicitly grants project-wide reads and cannot be
  combined with `service` or another key scope.

The explicit grant is persisted as `api_keys.allow_unbound`, returned in admin
key metadata, and enforced after authentication. The migration defaults the
column to false, so legacy unbound read keys fail closed and must be rotated.
Admin authority remains project-wide; responder-write remains service-bound.

The HTTP key API and the `mint-key` recovery command share this policy.
`mint-key` uses `--allow-unbound` for the exceptional project-wide reader.

## Redaction Boundary

The existing shared deterministic redaction engine remains the single
vocabulary. It now recognizes JWTs, AWS access-key IDs, private-key blocks,
GitHub and Slack tokens, common provider tokens, and credential-bearing
database URIs in addition to the existing Bearer, Canary-key, assignment, and
email rules. Ingest scrubs every persisted free-form error field before
grouping and storage; service remains an authorization identity and is not
silently rewritten.

This is a bounded secret-pattern floor, not a claim of general DLP. New
credential families extend the shared corpus and persistence/readback test.

## Consequences

- Operators must rotate legacy unbound read keys after upgrade.
- Deliberate fleet readers remain possible but are visible and auditable.
- Bound-key route behavior is unchanged: cross-service detail is hidden or
  forbidden according to each existing public route contract.
- False positives are limited to declared credential shapes; Canary does not
  redact generic high-entropy strings.
