# Signal-agnostic annotations — the missing Ramp primitive

Priority: medium
Status: blocked
Estimate: M

## Goal
Generalize Canary's annotations surface from "incidents only" to **any signal type** (`incident`, `error_group`, `target`, `monitor`), so a responder agent can stamp a PR reference, acknowledgement, or diagnosis onto the signal that originated the work — closing the ramp-pattern dedup loop without a second coordination store.

## Non-Goals
- Store agent prose, LLM outputs, or triage narratives as Canary-native content — annotations remain consumer-authored opaque JSON with a small required envelope (responder boundary)
- Prescribe annotation semantics (no enum of "valid actions"; consumers define meaning)
- Index annotation content for search (annotations are key-value attachments, not searchable records)
- Replace or deprecate the existing `/api/v1/incidents/{id}/annotations` route — it continues to work as a compatibility shim
- Cross-reference annotations across subject types in the same query (one subject at a time; agents aggregate client-side if needed)

## Oracle
- [ ] Schema migration introduces `subject_type` (enum of `incident | error_group | target | monitor`) and `subject_id` (string) columns on `annotations`, with a unique composite index on `(subject_type, subject_id, id)` — verified by migration round-trip in a fresh SQLite database
- [ ] `curl -s -X POST -H "X-API-Key: $KEY" -H "Content-Type: application/json" -d '{"subject_type":"error_group","subject_id":"<hash>","agent":"bb-triage","action":"linked","metadata":{"pr":"https://github.com/.../pull/123"}}' https://canary-obs.fly.dev/api/v1/annotations | jq -e '.id'` returns a new annotation id
- [ ] `curl -s -H "X-API-Key: $KEY" "https://canary-obs.fly.dev/api/v1/annotations?subject_type=error_group&subject_id=<hash>" | jq -e '.annotations | length > 0'` returns `true` after the POST above
- [ ] The incident-detail payload shipped in #023 inlines annotations keyed on `(incident, incident_id)` (backward compatible), and — separately — each `signals[].annotation_count` field is populated with the count of annotations on that signal's underlying `(error_group | target | monitor)` subject. A responder can see "this error-group has 3 prior annotations" without an extra GET.
- [ ] Webhook event `annotation.added` fires on creation for all subject types; payload includes `subject_type`, `subject_id`, and the annotation shape; signed via existing HMAC path; appears in `X-Delivery-Id` ledger
- [ ] Legacy endpoint `POST /api/v1/incidents/{incident_id}/annotations` still works; it writes an annotation with `subject_type=incident, subject_id=incident_id` — verified by keeping the existing controller test green after the schema migration
- [ ] Scoped-key policy: `write:annotations` scope gates `POST /annotations`; `read:incidents` / `read:errors` / `read:health` scopes gate reads of annotations for the corresponding subject types (verified in `test/canary_web/plugs/auth_scope_test.exs`)
- [ ] `priv/openapi/openapi.json` adds `POST /api/v1/annotations`, `GET /api/v1/annotations`, `AnnotationRequest`, `AnnotationsResponse`, and extends `Annotation` with `subject_type` + `subject_id`; existing annotation path kept as-is for compatibility
- [ ] `./bin/validate` green (fast + strict); coverage holds at 81% core / 90% canary_sdk; Oban `annotation.added` delivery jobs enqueue correctly (no race with `pool_size:1`)

## Notes

**Why now.** The 2026-04-21 grooming investigation Scout lens identified writable signal metadata as **the single missing Ramp-loop primitive** in Canary. Evidence:

1. **Ramp Labs' "self-maintaining" pattern (2026-03-23).**
   When the fix-agent produces a PR for an alert, it posts the PR URL *back onto the monitor description itself* as the dedup token. Next time the monitor fires, the triage agent reads the monitor description first; if a PR link is present and open, it skips the investigation and subscribes to the PR. This is how Ramp avoids the "ten agents investigate the same incident" failure mode *without* adding a separate coordination DB — the observability substrate **is** the coordination plane. Canary today has annotations, but only on incidents.

2. **Annotations are hard-coded to `incident_id` in the current schema.**
   `lib/canary/annotations.ex:10-29` — every function is `create_for_incident/2`, `list_for_incident/1`. The `Annotation` schema carries `incident_id` as its only subject FK. An agent that wants to stamp an error group or a monitor target with "PR #123 fixes this" has nowhere to write it. This forces downstream agents to maintain their own state store keyed on `(service, error_class, group_hash)` — exactly the coordination DB Ramp's pattern avoids.

3. **Incidents are the wrong granularity for dedup.**
   An incident is an ephemeral correlation; an error group persists across incidents (same root cause re-triggers). If a responder agent fixes the underlying error group, the "fixed" marker needs to live on the group, not on the incident that happened to be open at the time — otherwise the next incident that correlates the same error group loses the context.

**External anchor (Scout, cited in #023 as well).** Stripe / GitHub / Shopify webhook contracts publish 6–7 load-bearing headers. None of those platforms expose writable metadata on the underlying objects because their consumers are humans. Canary's agent-first posture inverts that: the substrate *must* be writable because the consumer is also a producer of coordination signal.

**Responder-boundary check.** This is a substrate capability, not a prescribed behavior. Canary owns the annotation surface (schema, endpoints, webhook event). Consumers own what they write into it. `PRINCIPLES.md` #3 "Broadcast, Don't Prescribe" holds: no enum of "valid actions," no opinionated workflow, no GitHub-shaped assumptions in the payload. An annotation is `{agent, action, metadata, created_at}` where `action` and `metadata` are free-form; Canary neither validates nor interprets them.

**Schema sketch.**

```
alter table :annotations,
  add :subject_type, :string, null: false, default: "incident"
  add :subject_id, :string, null: false
  -- backfill subject_id from incident_id for existing rows, then drop incident_id FK
  -- keep incident_id as a plain column (nullable) for the compatibility shim / legacy queries
create index :annotations, [:subject_type, :subject_id, :created_at]
create unique_index :annotations, [:subject_type, :subject_id, :id]
```

Subject-type enum: `incident`, `error_group`, `target`, `monitor`. Closed set; new types require an intentional migration. This bounds the blast radius and makes the query paths finite and predictable.

**Endpoint shape.**

- `POST /api/v1/annotations` — body: `{subject_type, subject_id, agent, action, metadata}` → returns full annotation
- `GET /api/v1/annotations?subject_type=&subject_id=&cursor=&limit=` — paginated, newest-first, bounded (max 50 per page)
- `POST /api/v1/incidents/{incident_id}/annotations` — compatibility shim; writes with `subject_type=incident`
- `GET /api/v1/incidents/{incident_id}/annotations` — compatibility shim; reads with `subject_type=incident`

`GET /api/v1/annotations` returns an `AnnotationsResponse` with `{summary, annotations, cursor}` — `summary` follows the invariant ("3 annotations on error_group abc; latest from bb-triage-sprite 4m ago").

**Webhook event.**

`annotation.added` already fires for incident annotations. Extend it to all subject types; payload includes `subject_type` + `subject_id`. Delivery via the existing signed-webhook + ledger path (`#012`). Agents listening for annotation events get the same at-least-once + `X-Delivery-Id` dedup guarantees.

**Interaction with #023.** The incident-detail endpoint (#023) already inlines incident-keyed annotations. After this ticket, it additionally surfaces per-signal `annotation_count` for the underlying error-group / target / monitor subjects. No payload shape break — just an additional integer per signal. A responder that wants to *read* those per-signal annotations calls `GET /annotations?subject_type=error_group&subject_id=...` as a follow-up; the count lets them decide whether that follow-up is worth making.

**Dependency.** Blocked on #023 for the detail-payload integration shape — without #023, there's no incident-detail response to thread `annotation_count` into. The schema/endpoint work can start in parallel with #023's controller work once #023's response shape is frozen; practically, serialize.

**Execution sketch (one PR, three atomic commits).**

*Commit 1 — `feat(db): generalize annotations to (subject_type, subject_id)`.*
Ecto migration: add columns, backfill `subject_id := incident_id, subject_type := "incident"` for existing rows, add indexes. Keep `incident_id` column for compatibility (nullable, populated for legacy rows). Update `Canary.Annotations.Annotation` schema. `@required`/`@optional` lists updated — remembering the custom string PK footgun (`CLAUDE.md`), annotation IDs are still set on the struct, not cast.

*Commit 2 — `feat(annotations): add subject-agnostic endpoints and context functions`.*
New controller `CanaryWeb.AnnotationController`. New context functions `Canary.Annotations.create/1`, `Canary.Annotations.list/2`. Wire the compatibility shims. Extend `priv/openapi/openapi.json`.

*Commit 3 — `feat(incidents): surface per-signal annotation counts in incident detail`.*
Augment `Canary.Query.Incidents.detail/2` (from #023) to join `annotations` grouped by `(subject_type, subject_id)` and populate `signals[].annotation_count`. One extra query (stays within the #023 query budget of ≤4 by combining with the signals join). Update integration test.

**Risk list.**

- *Schema migration on live SQLite WAL-mode database* → additive columns + backfill; safe under the single-writer model. Rehearse on a production-sized fixture first.
- *Enum drift over time (new subject types)* → intentional: adding a subject type is a schema migration + controller validation update. The friction is a feature, not a bug — prevents accidental proliferation.
- *Write amplification from annotation webhooks on high-traffic surfaces* → rate limited via existing `Canary.Errors.RateLimiter` (or its post-#022 equivalent) scoped on `agent` + `subject_id`.
- *Annotation count staleness in the #023 detail payload* → acceptable; eventually consistent under the single-writer model, and bounded by the same query that fetches signals.
- *Legacy consumers calling `POST /incidents/{id}/annotations`* → compatibility shim preserved indefinitely; no deprecation signal emitted until at least two responder agents are in production and confirmed to use the new endpoint.

**Lane.** Lane 1 (agent readiness) — completes the Canary-side substrate for the ramp pattern (`backlog.d/010`). After this + #023, the bitterblossom triage sprite (`bb/011`) has everything it needs from the observability layer: one-call incident context, and a writable metadata surface it can stamp as it works. The responder boundary holds: Canary owns the read/write surfaces, the sprite owns the content and the mutation decisions.

Source: grooming session 2026-04-21. Parallel investigator evidence:
- Scout (the Ramp primitive; load-bearing recommendation).
- Strategist (annotation inlining as a friction-reducer in the incident-detail flow; informs the #023-integration commit).
- Archaeologist (confirmed the current incident-only schema; `lib/canary/annotations.ex` hardcodes `incident_id`).
