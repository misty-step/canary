# Incident as the atomic agent unit — detail endpoint + cross-references

Priority: high
Status: ready
Estimate: M

## Goal
A responder agent receiving an `incident.opened` webhook can, in **one** authenticated GET against `/api/v1/incidents/{id}`, obtain a bounded payload that contains the incident's summary, all correlated signals (error groups + monitor/target transitions), inlined annotations, and the latest timeline events — enough context to decide whether to act without any follow-up query.

## Non-Goals
- Implement LLM triage, fix suggestions, or any prose generation beyond the existing deterministic-template `summary` pattern (responder boundary — that work lives in bitterblossom `bb/011`)
- Change how incidents are **created** or correlated (`lib/canary/incidents/` correlation logic stays as-is)
- Open a repo-mutation or issue-creation surface on the Canary side (responder boundary)
- Add a dashboard view for the detail (this is an API surface; operators use `curl | jq`)
- Expand webhook payloads in this ticket — webhook enrichment is a candidate follow-up, not blocking

## Oracle
- [ ] `curl -s -H "X-API-Key: $KEY" https://canary-obs.fly.dev/api/v1/incidents/INC-<id> | jq -e '.summary and .signals and .annotations and .recent_timeline_events'` returns `true` for a known real incident
- [ ] Response payload is bounded: `signals` capped at 25 entries (with `signals_truncated: bool` + `cursor`), `annotations` capped at 20 (newest-first), `recent_timeline_events` capped at 5 — verified by an integration test that inserts 100 of each and asserts the caps
- [ ] Each entry in `signals` carries enough context to act on without a follow-up query: `{type: "error_group" | "health_transition" | "monitor_transition", summary: <string>, ...type-specific-fields}` — asserted in controller test
- [ ] `GET /api/v1/incidents` (list) response includes a top-level `summary` field (natural-language synthesis across the returned page) — restoring parity with `StatusResponse` / `ReportResponse` / `TimelineResponse`
- [ ] `GET /api/v1/errors/{id}` response includes `incident_ids: [String.t()]` (possibly empty) — asserted by integration test that correlates an error into an incident and then fetches it
- [ ] `priv/openapi/openapi.json` has `/api/v1/incidents/{incident_id}` path with `IncidentDetailResponse` schema; `IncidentsResponse` schema adds `summary`; `ErrorDetailResponse` schema adds `incident_ids` — all three changes captured in the OpenAPI diff
- [ ] `info.x-agent-guide` in `priv/openapi/openapi.json` names the incident detail endpoint as the canonical "start here on webhook" call — the guidance reads as one sentence, not a tutorial
- [ ] `mix test test/canary_web/controllers/incident_controller_test.exs` covers: happy path, 404 for unknown id, scoped-key denial (read-incidents scope absent), pagination via `signals` cursor, bounded payload under a 100-signal incident
- [ ] `./bin/validate` green (fast + strict lanes); coverage holds at 81% core / 90% canary_sdk; no new n+1 query introduced (verified by enabling Ecto log in test and counting queries — should be ≤ 4 per request: incident + signals + annotations + timeline)

## Notes

**Why now.** The 2026-04-21 grooming investigation named this the single highest-impact agent-first gap. Three lines of evidence:

1. **The "one call to understand everything" principle is structurally absent.**
   - `priv/openapi/openapi.json:710` defines `/api/v1/incidents` (list). Line 755 defines `/api/v1/incidents/{incident_id}/annotations`. There is **no** `/api/v1/incidents/{incident_id}` detail route.
   - A responder agent receiving `incident.opened` currently must make: (1) `GET /incidents?...` to find the incident, (2) `GET /incidents/{id}/annotations` to check prior triage, (3) `GET /timeline?service=...` or `GET /query?service=...` to see signals. Three calls, three context-window hits, before the agent can decide whether to act.
   - `PRINCIPLES.md` #1 is explicit: *"Response payloads are bounded"* and *"No information requires clicking through a dashboard to understand."* Requiring three API calls is the query-API analogue of clicking through a dashboard.

2. **`IncidentsResponse` violates the `summary` invariant.**
   `priv/openapi/openapi.json:2627-2641` — `IncidentsResponse` schema has `required: ["incidents"]` and no `summary` property. Every other bounded list response (`StatusResponse`, `ReportResponse`, `TimelineResponse`) includes `summary`. This mirrors the `errors_by_class` drift addressed in #022 — same invariant, different surface.

3. **Error↔incident linkage is one-way.**
   `priv/openapi/openapi.json:2167-2184` — `ErrorDetailResponse` has `required: ["summary", "id", "service", "error_class", "message", "message_template", "stack_trace", "context", "severity", "environment", "group_hash", "created_at", "group"]`. No `incident_id` or `incident_ids`. The correlation engine links errors *into* incidents, but the error-detail query can't answer "which incidents reference me?" Agents working from an error group have to reverse-engineer incident membership by filtering the timeline — a second-class experience that reveals the model: incidents are the correlation root, but the API treats them as a sibling concept.

**External anchor (Scout).** Ramp Labs' "self-maintaining" post (2026-03-23) identifies the observability substrate's atomic unit as the *monitor-as-shared-state* — the single addressable thing that consumer agents coordinate around. Canary's equivalent is the incident. Making it one-call-atomic is the substrate-side move that unblocks the ramp loop.

**Response shape (authoritative — matches what the controller will build).**

```json
{
  "summary": "Investigating. High-severity incident opened 4m ago on service api. Correlates 2 error-group regressions and 1 health-check failure. No prior triage annotations.",
  "incident": {
    "id": "INC-01H...",
    "service": "api",
    "state": "investigating",
    "severity": "high",
    "title": "...",
    "opened_at": "2026-04-21T19:14:02Z",
    "resolved_at": null,
    "signal_count": 3
  },
  "signals": [
    {
      "type": "error_group",
      "summary": "38 occurrences of Elixir.Ecto.ConstraintError in 4m, new in last 24h",
      "group_hash": "abc123...",
      "error_class": "Elixir.Ecto.ConstraintError",
      "total_count": 38,
      "first_seen_at": "...",
      "last_seen_at": "..."
    },
    {
      "type": "health_transition",
      "summary": "Target api-primary transitioned healthy → down after 3 consecutive failures",
      "target_id": "TGT-...",
      "from_state": "healthy",
      "to_state": "down",
      "occurred_at": "...",
      "consecutive_failures": 3
    }
  ],
  "signals_truncated": false,
  "signals_cursor": null,
  "annotations": [
    {
      "id": "ANN-...",
      "incident_id": "INC-...",
      "agent": "bb-triage-sprite",
      "action": "acknowledged",
      "metadata": { "...": "..." },
      "created_at": "..."
    }
  ],
  "recent_timeline_events": [
    { "type": "incident.opened", "occurred_at": "...", "summary": "..." }
  ]
}
```

Bounded caps: `signals ≤ 25`, `annotations ≤ 20` (newest-first), `recent_timeline_events ≤ 5`. Target: the full payload fits in ~3–5k tokens for typical incidents, ~8k worst-case at caps.

**Responder-boundary check.** All three changes (detail endpoint, list `summary`, `incident_ids` backlink) are strictly on the substrate side — read-only query surfaces that expose existing correlation state. No consumer-facing behavior is prescribed. An agent reading an annotation like `{agent: "bb-triage-sprite", action: "acknowledged"}` decides what to do with that information; Canary just reports it.

**Dependency on #022.** Weak dependency — ideally `errors_by_class` has `summary` landed before we advertise the `summary`-on-every-response invariant harder via the agent guide. But this ticket can technically ship independently; the dependency is editorial, not structural.

**Execution sketch (one PR, three atomic commits).**

*Commit 1 — `feat(query): add summary to IncidentsResponse and incident_ids to ErrorDetailResponse`.*
List-level `summary` is a deterministic template over the returned page ("N open incidents across M services; newest: ..."). `incident_ids` comes from the existing correlation table — no schema change needed, just join + select. Both changes additive; no consumer break.

*Commit 2 — `feat(query): add GET /api/v1/incidents/{id} with bounded detail`.*
New controller action `incident_controller.ex:show/2`. New read-model function `Canary.Query.Incidents.detail/2` returning the payload above. Enforce caps in the read model, not the controller. Scoped-key requirement: `read:incidents`. Integration test for pagination, 404, caps. OpenAPI schema addition: `IncidentDetailResponse`.

*Commit 3 — `docs(openapi): name incident detail as canonical "start here" for webhook consumers`.*
Update `priv/openapi/openapi.json` `info.x-agent-guide` to read something like: *"On `incident.opened` webhook, call `GET /api/v1/incidents/{id}` — one call returns summary, signals, annotations, and recent timeline. Only fall through to `/query`, `/timeline`, or `/errors/{id}` if the detail payload is truncated."*

**Risk list.**

- *N+1 query risk when assembling the payload* → enforce a query budget of ≤4 (one each for incident, signals, annotations, timeline). Add an Ecto query-count assertion to the integration test. The read-model split from #006 makes this straightforward.
- *Signal heterogeneity makes the payload sprawl* → the `type`-tagged union shape (`error_group | health_transition | monitor_transition`) constrains it. Document the tag set in OpenAPI; agents get a closed set, not an open-ended blob.
- *Annotation inlining could become unbounded on high-traffic incidents* → hard cap at 20 newest; if more exist, `annotations_truncated: true` + pointer to existing `/incidents/{id}/annotations` paginated endpoint.
- *Monitor/target transitions may not be correlated to incidents today* → verify during implementation. If correlation doesn't exist yet, the `signals` field will contain only `error_group` entries for monitor-originated incidents. Ticket should document this honestly and file a follow-up if the gap is real.

**Lane.** Lane 1 (agent readiness). Directly advances the Canary-side of the ramp pattern (`backlog.d/010`) — the bitterblossom triage sprite (`bb/011`) will consume this endpoint as its primary context-load call. This item does not unblock `010` on its own (bb/011 is the gating work), but it is what `bb/011` needs to be able to finish.

Source: grooming session 2026-04-21. Parallel investigator evidence:
- Strategist (all five findings; recommendation text lightly adapted here for self-containment).
- Scout (Ramp monitors-as-shared-state; Stripe-class single-shot payload discipline).
- Archaeologist (IncidentsResponse `summary` gap via the same invariant lens as #022's `errors_by_class` finding).
