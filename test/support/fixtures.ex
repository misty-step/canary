defmodule Canary.Fixtures do
  @moduledoc "Shared test data builders."

  def clean_status_tables do
    Canary.Repo.delete_all(Canary.Schemas.ServiceEvent)
    Canary.Repo.delete_all(Canary.Schemas.IncidentSignal)
    Canary.Repo.delete_all(Canary.Schemas.Incident)
    Canary.Repo.delete_all(Canary.Schemas.TargetState)
    Canary.Repo.delete_all(Canary.Schemas.TargetCheck)
    Canary.Repo.delete_all(Canary.Schemas.Target)
    Canary.Repo.delete_all(Canary.Schemas.Error)
    Canary.Repo.delete_all(Canary.Schemas.ErrorGroup)
  end

  def create_target_with_state(name, state) do
    id = "TGT-#{name}"
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    Canary.Repo.insert!(%Canary.Schemas.Target{
      id: id,
      name: name,
      service: name,
      url: "https://#{name}.example.com/healthz",
      created_at: now
    })

    Canary.Repo.insert!(%Canary.Schemas.TargetState{
      target_id: id,
      state: state,
      consecutive_failures: if(state == "up", do: 0, else: 3),
      last_checked_at: now,
      last_success_at: if(state == "up", do: now, else: nil)
    })
  end

  def create_error_group(service, error_class, count, opts \\ [])
      when count > 0 do
    last_seen_at = Keyword.get(opts, :last_seen_at, DateTime.utc_now() |> DateTime.to_iso8601())
    id = "ERR-#{:crypto.strong_rand_bytes(8) |> Base.url_encode64(padding: false)}"

    group_hash =
      :crypto.hash(:sha256, "#{service}:#{error_class}") |> Base.encode16(case: :lower)

    Canary.Repo.insert!(%Canary.Schemas.ErrorGroup{
      group_hash: group_hash,
      service: service,
      error_class: error_class,
      severity: "error",
      status: "active",
      first_seen_at: last_seen_at,
      last_seen_at: last_seen_at,
      total_count: count,
      last_error_id: id
    })
  end
end
