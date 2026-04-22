# OpenAPI ↔ Ecto type-parity Dagger lane

Priority: medium
Status: ready
Estimate: M

## Goal

Add a `contract` Dagger lane that, given `priv/openapi/openapi.json`
and the compiled `Canary.Schemas.*` modules, asserts every schema
field referenced through the OpenAPI surface uses a JSON type
compatible with its Ecto declaration. Ships as part of
`./bin/validate --strict` so CodeRabbit-class "integer in OpenAPI
where the Ecto PK is a string nanoid" mismatches fail CI before merge.

## Non-Goals

- Build a full Ecto-to-OpenAPI code generator. The OpenAPI source
  is the authoritative contract; this lane only verifies the
  *overlap* where both layers describe the same field is
  type-consistent.
- Validate request bodies against schemas (that's the job of a
  runtime response-validation middleware — different work).
- Ban OpenAPI features that have no Ecto analogue (e.g. `oneOf`
  unions, synthetic summary fields).

## Oracle

- [ ] `./bin/validate --strict` fails with a structured diagnostic
      when a test fixture OpenAPI schema declares an integer
      property whose Canary schema counterpart is a string
      (e.g. a nanoid `id`)
- [ ] `./bin/validate --strict` passes on the post-#023 tree where
      `IncidentDetailTimelineEvent.id` was fixed to `string`
- [ ] The lane's output is actionable: names the OpenAPI component,
      the JSON-pointer of the offending field, the Ecto schema +
      field, and the expected vs. actual JSON type
- [ ] New Dagger function `contract()` in `dagger/src/index.ts`
      invoked from `strict()`; described under
      `docs/ci-control-plane.md` in the lane matrix
- [ ] The implementation is a Mix task (`mix canary.openapi.parity`)
      that the Dagger function calls — so developers can run it
      locally without Dagger
- [ ] Coverage holds at 81% core / 90% canary_sdk; no new footgun
      introduced in `CLAUDE.md`

## Notes

**Why now.** PR #133 shipped a new OpenAPI schema
(`IncidentDetailTimelineEvent`) that declared `id` as `type: integer`
while `Canary.Schemas.ServiceEvent.id` is a string nanoid. The
`openapi_controller_test.exs` "additionalProperties" discipline test
did not catch it — that test is a structural shape check, not a type
check. CodeRabbit flagged the mismatch via diff inspection. Systematic
prevention needs a dedicated lane that crawls both artifacts.

**Detection shape.**

Mix task `mix canary.openapi.parity`:

1. Load `priv/openapi/openapi.json`.
2. Walk `components.schemas` — for each object schema, collect
   `{component, property, declared_type}` triples.
3. Heuristically map OpenAPI components to Ecto schemas using a
   rule table maintained in
   `priv/openapi/parity_map.exs`:
   ```elixir
   %{
     "Incident" => Canary.Schemas.Incident,
     "IncidentSignal" => Canary.Schemas.IncidentSignal,
     "IncidentDetailIncident" => Canary.Schemas.Incident,
     "IncidentDetailTimelineEvent" => Canary.Schemas.ServiceEvent,
     "ErrorDetailResponse" => Canary.Schemas.Error,
     "ErrorGroupSummary" => Canary.Schemas.ErrorGroup,
     # ...
   }
   ```
   Components not in the map are skipped with a single WARN log
   line — parity is opt-in per component, not implicit.
4. For each mapped component, cross-check the properties that share a
   name with an Ecto field against the field's Ecto type. Use this
   compatibility table:
   - Ecto `:string` → OpenAPI `"string"` or `["string", "null"]`
   - Ecto `:integer` → OpenAPI `"integer"` or `["integer", "null"]`
   - Ecto `:boolean` → OpenAPI `"boolean"`
   - Ecto `:map` / `:array` / `{:array, _}` → OpenAPI `"object"` /
     `"array"` respectively, or a `$ref`
   - Custom string PKs (`:string` with `autogenerate: false`) — must
     NOT be OpenAPI `"integer"`. This is the specific CodeRabbit hit.
5. Emit mismatches to stdout with the file, JSON pointer, and
   recommended fix. Non-zero exit on any mismatch.

**Dagger wiring.**

New function in `dagger/src/index.ts` (sketch):

```ts
@func()
async contract(source: Directory): Promise<string> {
  return await this.mixContext(source)
    .withExec(["mix", "canary.openapi.parity"])
    .stdout()
}
```

Called from `strict()`:

```ts
await this.contract(repo)
```

**Execution sketch (one PR, three commits).**

*Commit 1 — `feat(contract): add mix canary.openapi.parity task`.*
New Mix task under `lib/mix/tasks/canary.openapi.parity.ex`.
Parity map at `priv/openapi/parity_map.exs`. Unit-tested against
synthetic fixtures under
`test/mix/tasks/openapi_parity_test.exs` including the positive
case, a string-vs-integer mismatch, and an unmapped-component skip.

*Commit 2 — `feat(ci): add contract Dagger lane to strict gate`.*
Add `contract()` function to `dagger/src/index.ts`, wire into
`strict()`. Update `docs/ci-control-plane.md` lane matrix.
Regenerate any pinned snapshots.

*Commit 3 — `docs(openapi): cross-reference the parity gate`.*
`priv/openapi/README.md` (new, small) describes the parity map and
how to add a new component binding. Link from `VISION.md` under
"What Canary Is" to signal: the OpenAPI spec is the agent contract,
and it's enforced.

**Risk list.**

- *Parity map rots when new schemas land without a map entry.* The
  task defaults to WARN (not FAIL) on unmapped components so a single
  new schema can't red-line CI before the author writes the binding.
  But a separate strict-only assertion fails if unmapped components
  exceed a threshold (say, 3). Tunable.
- *Ecto type introspection requires compiled modules.* The Mix task
  runs in the Mix env; the Dagger lane wraps it in the same container
  as `mix credo`. No new build-time dependency.
- *Some OpenAPI types are legitimately broader than Ecto.* For
  example, the Annotation `metadata` field is Ecto `:map` (stored as
  JSON string) but OpenAPI represents it as `oneOf: [object, string,
  null]`. The compatibility table must accept the union form.

**Lane.** Lane 4 (hardening). Depends on #026 and #027 being landed
first only because they share the Credo infrastructure feel; technically
independent.

**Share with spellbook?** The *pattern* (contract-source ↔ storage-source
parity check as a dedicated CI lane) generalizes. The *implementation*
is bound to Phoenix + Ecto + SQLite3 ecto adapter. Spellbook-side
reference under `#048` captures the pattern; this ticket ships the
canary implementation.

Source: `/reflect prevent-coderabbit-patterns` 2026-04-21, CodeRabbit
finding on PR #133 (comment id 3120641552).
