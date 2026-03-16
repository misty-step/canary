defmodule Canary.Fixtures do
  @moduledoc "Shared test data builders."

  def create_target_with_state(name, state) do
    id = "TGT-#{name}"
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    Canary.Repo.insert!(%Canary.Schemas.Target{
      id: id,
      name: name,
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

  def create_error_group(service, error_class, count) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    group_hash =
      :crypto.hash(:sha256, "#{service}:#{error_class}") |> Base.encode16(case: :lower)

    for i <- 1..count do
      id = "ERR-#{:crypto.strong_rand_bytes(8) |> Base.url_encode64(padding: false)}"

      Canary.Repo.insert!(
        %Canary.Schemas.Error{
          id: id,
          service: service,
          error_class: error_class,
          message: "#{error_class}: something failed",
          message_template: "#{error_class}: something failed",
          severity: "error",
          environment: "production",
          group_hash: group_hash,
          created_at: now
        },
        on_conflict: :nothing
      )

      if i == 1 do
        Canary.Repo.insert!(%Canary.Schemas.ErrorGroup{
          group_hash: group_hash,
          service: service,
          error_class: error_class,
          severity: "error",
          status: "active",
          first_seen_at: now,
          last_seen_at: now,
          total_count: count,
          last_error_id: id
        })
      end
    end
  end
end
