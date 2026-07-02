# Incidents as the unifying primitive; apps-first information architecture

Priority: P1 · Status: pending · Estimate: XL

## PRD Summary
- User: the operator and any human viewing Canary's dashboard/UI across the
  fleet of monitored applications.
- Problem: today's surfaces are organized around signal *sources* (errors,
  health-check up/down walls, downtime) rather than around the applications
  those signals describe, and an incident's detail view does not yet read as
  a self-contained emergency work ticket with maximum context.
- Goal: reorganize the top-level information architecture around monitored
  applications, treat errors/failed health checks/downtime as incident
  *types* from different sources rather than separate primary views, and
  make incident detail an emergency work ticket carrying full origin context
  plus an agent-written work log.
- Why now: Canary already has typed incidents (auto-correlated from health
  and error signals), remediation claims, annotations, and telemetry
  events; the missing piece is presenting them through an apps-first IA
  instead of source-siloed tabs, and enriching incident detail into a
  standalone work record.
- UX enabled: an operator or agent opens an application, sees its uptime,
  24h/7d error counts, and incident stream in one place, drills into any
  incident, and gets everything needed to work it — origin payload, stack
  trace or health-check detail, and a running log of hypotheses tried,
  actions taken, PRs opened, and proof of resolution — without leaving the
  page or cross-referencing Powder.
- Deliverable type: data-model unification, new read/aggregation surfaces,
  and a UI rebuild; contract docs and fixtures for the incident-type source
  unification.
- Success signal: the dashboard's top level lists applications (not four
  source-specific tabs), each incident detail view is sufficient on its own
  to work the incident end to end, and global cross-app metrics remain
  available alongside the per-app drill-down.

## Product Requirements
- P0: top-level navigation is monitored applications — per-app uptime, 24h
  and 7d error counts, and an incident stream — plus a global metrics view.
- P0: errors, failed health checks, and downtime are modeled and presented
  as incident *types* from different signal sources, not as separate
  primary up/down-wall views.
- P0: incident detail carries origin context (stack trace for errors,
  health-check detail for probe/monitor failures) plus an agent-written work
  log (hypotheses validated/invalidated, actions taken, PR references, proof
  of resolution, created/resolved timestamps).
- P1: a global incident feed exists alongside the apps → app → incident
  drill-down (two navigation modes, not four source tabs).
- P1: the incident work-log fields deliberately partially duplicate Powder's
  ticket/scratchpad semantics (work notes, hypotheses, actions); this
  duplication is accepted, not treated as a bug, though it stays open for
  debate.
- Non-goals: this epic does not redesign Powder's ticket model, does not
  change the incident state machine correlation logic (`incidents.rs`)
  itself, and does not remove or replace the escalation overlay
  (`incident_escalation.rs`/`escalation.rs`) already shipping — see Children
  item 5.

## Technical Design
- Chosen architecture: keep incidents as the correlated, typed entity they
  already are (auto-correlated from health/error signals); add an explicit
  "source/type" facet on incident read models so errors/health/downtime
  render as classified incident types rather than requiring separate
  top-level endpoints. Introduce an application-scoped aggregation read
  model (uptime + windowed error counts + incident stream per service) that
  existing per-service and global report/query endpoints can back.
- Files/systems touched: `crates/canary-core/src/query.rs` and incident
  types, `crates/canary-store` read models (incidents, reports), a new or
  extended per-app aggregation endpoint, OpenAPI (`priv/openapi/openapi.json`),
  the human dashboard UI (per commit `6d9c343`), and MCP/CLI surfaces that
  expose incident/report data.
- Data/control flow: existing signal ingestion (errors, health probes,
  monitor check-ins) continues to correlate into incidents unchanged; the
  new work is presentation and aggregation — grouping incidents and error/
  health counts by application for the top-level view, and enriching
  incident detail responses with origin payload + work-log fields.
- Build/check boundary: contract tests proving the app-scoped aggregation
  read model matches live incident/error/health state; OpenAPI schema tests
  for the new incident-type/work-log fields; dashboard route/read tests; UI
  screenshot evidence for the apps-first drill-down and global feed.
- ADR decision: required if incident work-log storage introduces a new
  entity/table rather than extending existing incident/annotation storage.
- ADR-style invariants: the incident state machine (`incidents.state`)
  remains a pure function of signal activity — this epic must not let
  work-log or app-aggregation concerns leak into that computation, same
  invariant the escalation overlay already respects.
- Design X vs Y: prefer classifying existing incident types over building a
  parallel apps-first data model; prefer an aggregation read model layered
  on existing incident/error/health/monitor stores over introducing a new
  "Application" write-side entity, unless investigation shows `service` is
  insufficient as the application identity key.

## Goal
Reorganize Canary's information architecture around monitored applications
rather than signal-source silos, and make each incident's detail view a
maximum-context emergency work ticket sufficient to resolve the incident
without leaving the page.

## Oracle
- [ ] Given an operator opens Canary's top level, then they see a list of
      monitored applications, each showing uptime, 24h error count, 7d error
      count, and an incident stream — not four source-specific tabs.
- [ ] Given errors, failed health checks, and downtime occur for a service,
      then they are surfaced as incident *types* within that service's
      unified incident stream, not as separate up/down-wall primary views.
- [ ] Given an operator drills into one application, then they see that
      app's dedicated view before drilling further into a specific incident
      (apps → app → incident navigation).
- [ ] Given an operator opens an incident, then the detail view includes
      origin context (stack trace or health-check payload) and an
      agent-written work log (hypotheses, actions, PR references, proof of
      resolution, created/resolved timestamps) sufficient to work the
      incident without cross-referencing another system.
- [ ] Given global visibility is needed, then a global incident feed and
      global metrics view exist alongside the per-app drill-down.
- [ ] Given the escalation overlay (`incident_escalation.rs`/`escalation.rs`)
      is already shipping, then this epic's Children explicitly reconcile
      with it rather than silently duplicating or conflicting with its
      state.

## Verification System
- Claim: an operator or agent can understand and act on any incident from
  one application-scoped view without stitching together separate
  error/health/downtime dashboards.
- Falsifier: an incident's type is only inferable by checking which
  source-specific view it appeared in, or an incident detail view is
  missing origin context or work-log history that exists elsewhere in the
  system.
- Driver: app-aggregation read-model contract tests, incident-type
  classification tests, dashboard route tests, and UI screenshot evidence
  of the apps-first drill-down plus global feed.
- Grader: aggregation counts match live incident/error/health state;
  incident detail responses carry both origin payload and work-log fields;
  screenshots show apps-first navigation, not source-siloed tabs.
- Evidence packet: route-test names, OpenAPI diff, and a dashboard
  screenshot set (light/dark, apps list, app detail, incident detail, global
  feed) attached to the PR.
- Cadence: strict gate for contract/route tests; screenshot evidence at UI
  rebuild time.

## Notes
Operator directive, verbal, 2026-07-02. Top level = monitored applications
(per-app: uptime, error counts 24h/7d, incident stream; global metrics too).
Errors, failed health checks, downtime — all just incident *types* from
different sources; up/down walls are not the primary view. Incident detail =
an emergency work ticket with maximum context: origin context (stack trace,
health-check info) plus an agent-written work log (hypotheses
validated/invalidated, actions, PR, proof of resolution, created/resolved
timestamps) — a deliberate partial duplication of Powder semantics, accepted
("duplication is okay, up for debate"). UI: drill-down (apps → app →
incident) plus a global incident feed; not four tabs.

## Children
1. Incident-type unification in the data model — classify errors, failed
   health checks, and downtime as incident types from different sources
   within the existing correlated-incident model (`incidents.rs`); no change
   to the correlation state machine itself.
2. Per-app and global metrics surfaces — an application-scoped aggregation
   read model (uptime, 24h/7d error counts, incident stream) plus a global
   metrics/incident-feed view.
3. Incident context enrichment — capture and surface origin payload (stack
   trace, health-check detail) and add agent-writable work-log fields
   (hypotheses, actions, PR references, proof of resolution) to incident
   detail.
4. UI rebuild per the winning r2 design — apps list → app detail → incident
   detail drill-down, plus a global incident feed; replace source-siloed
   tabs.
5. Relationship-to-escalation-overlay note — reconcile this epic's incident
   detail/work-log shape with the already-shipping escalation overlay
   (`incident_escalation.rs` / `escalation.rs`); escalation stays an
   orthogonal layer on top of incident state per its existing module
   contract, not something this epic's UI or work-log rework should
   duplicate or destabilize.
