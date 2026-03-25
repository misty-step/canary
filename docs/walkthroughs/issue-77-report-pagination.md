# Issue 77 Walkthrough: Token-Efficient Report Responses

## Scenario

Verify that `/api/v1/report` now supports:

- bounded JSON pages with `limit`
- opaque cursor pagination without duplicates
- CSV serialization for `targets` and `error_groups`

## Commands

```bash
mix test test/canary/report_test.exs test/canary_web/controllers/report_controller_test.exs
mix test
mix credo --strict
mix dialyzer
```

Runtime spot check:

```bash
mix ecto.create && mix ecto.migrate
mix run -e 'Application.ensure_all_started(:canary); {:ok, _key, raw_key} = Canary.Auth.generate_key("runtime-check"); IO.puts(raw_key)'
mix phx.server
```

Then, with the emitted key:

```bash
curl -X POST http://127.0.0.1:4000/api/v1/targets \
  -H "Authorization: Bearer $KEY" \
  -H "Content-Type: application/json" \
  -d '{"name":"alpha","url":"https://example.com/","interval_ms":60000}'

curl -X POST http://127.0.0.1:4000/api/v1/errors \
  -H "Authorization: Bearer $KEY" \
  -H "Content-Type: application/json" \
  -d '{"service":"alpha","error_class":"ConnectionError","message":"database unavailable"}'

curl -H "Authorization: Bearer $KEY" \
  -H "Accept: text/csv" \
  "http://127.0.0.1:4000/api/v1/report?limit=5"
```

## Expected Output

JSON report responses include top-level pagination metadata:

```json
{
  "truncated": true,
  "cursor": "<opaque-token>"
}
```

CSV responses include one header row and rows for both sections:

```csv
section,position,id,name,service,error_class,url,state,count,first_seen,last_seen,severity,status,consecutive_failures,last_checked_at,cursor,truncated
targets,1,TGT-...,alpha,,,https://example.com/,unknown,,,,,,0,,,false
error_groups,1,,,alpha,ConnectionError,,active,1,2026-03-25T...,2026-03-25T...,error,active,,,,false
```

## Persistent Verification

- `test/canary/report_test.exs`
- `test/canary_web/controllers/report_controller_test.exs`

These tests cover default truncation, cursor paging, duplicate prevention, and CSV negotiation.
