defmodule Canary.Workers.TlsScanTest do
  use Canary.DataCase

  import Canary.Fixtures

  alias Canary.Schemas.{ServiceEvent, Target, TargetCheck}
  alias Canary.Workers.TlsScan

  setup do
    clean_status_tables()
    :ok
  end

  test "records timeline events for expiring tls certificates" do
    now = DateTime.utc_now() |> DateTime.to_iso8601()
    expiry = DateTime.utc_now() |> DateTime.add(7, :day) |> DateTime.to_iso8601()

    target =
      Repo.insert!(%Target{
        id: "TGT-api",
        name: "api-web",
        service: "api",
        url: "https://api.example.com/healthz",
        created_at: now
      })

    Repo.insert!(%TargetCheck{
      target_id: target.id,
      checked_at: now,
      result: "success",
      tls_expires_at: expiry
    })

    assert :ok = TlsScan.perform(%Oban.Job{})

    event =
      Repo.one!(
        from(e in ServiceEvent,
          where: e.event == "health_check.tls_expiring" and e.entity_ref == ^target.id
        )
      )

    payload = Jason.decode!(event.payload)

    assert event.service == "api"
    assert payload["target"]["service"] == "api"
    assert payload["tls_expires_at"] == expiry
  end
end
