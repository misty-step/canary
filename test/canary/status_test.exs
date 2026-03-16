defmodule Canary.StatusTest do
  use Canary.DataCase

  alias Canary.Status

  setup do
    # Clear pre-existing data (Health.Manager boot) within sandbox
    Canary.Repo.delete_all(Canary.Schemas.TargetState)
    Canary.Repo.delete_all(Canary.Schemas.TargetCheck)
    Canary.Repo.delete_all(Canary.Schemas.Target)
    Canary.Repo.delete_all(Canary.Schemas.ErrorGroup)
    :ok
  end

  describe "combined/0" do
    test "all healthy targets and no errors" do
      # Create 3 healthy targets
      for name <- ["alpha", "bravo", "charlie"] do
        create_target_with_state(name, "up")
      end

      result = Status.combined()

      assert result.overall == "healthy"
      assert length(result.targets) == 3
      assert Enum.all?(result.targets, &(&1.state == "up"))
      assert result.error_summary == []
      assert is_binary(result.summary)
      assert result.summary =~ "All 3 targets healthy"
    end

    test "target down with errors for that service" do
      create_target_with_state("volume", "down")
      create_target_with_state("api", "up")

      # Ingest errors for "volume" service
      for _ <- 1..12, do: create_error("volume", "ConnectionError")

      result = Status.combined()

      assert result.overall == "unhealthy"

      unhealthy = Enum.filter(result.targets, &(&1.state != "up"))
      assert length(unhealthy) == 1
      assert hd(unhealthy).name == "volume"

      volume_errors = Enum.find(result.error_summary, &(&1.service == "volume"))
      assert volume_errors.total_count == 12
      assert is_binary(result.summary)
      assert result.summary =~ "volume"
    end

    test "no targets and no errors" do
      result = Status.combined()

      assert result.overall == "empty"
      assert result.targets == []
      assert result.error_summary == []
      assert result.summary =~ "No services configured"
    end

    test "degraded target without errors" do
      create_target_with_state("api", "degraded")
      create_target_with_state("web", "up")

      result = Status.combined()

      assert result.overall == "degraded"
      assert result.summary =~ "degraded"
    end

    test "errors exist but all targets healthy" do
      create_target_with_state("api", "up")
      for _ <- 1..5, do: create_error("api", "TimeoutError")

      result = Status.combined()

      assert result.overall == "warning"
      assert result.summary =~ "error"
    end
  end

  # --- Helpers ---

  defp create_target_with_state(name, state) do
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

  defp create_error(service, error_class) do
    id = "ERR-#{:crypto.strong_rand_bytes(8) |> Base.url_encode64(padding: false)}"
    now = DateTime.utc_now() |> DateTime.to_iso8601()
    group_hash = :crypto.hash(:sha256, "#{service}:#{error_class}") |> Base.encode16(case: :lower)

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

    # Upsert the error group
    case Canary.Repos.read_repo().get(Canary.Schemas.ErrorGroup, group_hash) do
      nil ->
        Canary.Repo.insert!(%Canary.Schemas.ErrorGroup{
          group_hash: group_hash,
          service: service,
          error_class: error_class,
          severity: "error",
          first_seen_at: now,
          last_seen_at: now,
          total_count: 1,
          last_error_id: id
        })

      group ->
        group
        |> Ecto.Changeset.change(%{
          total_count: group.total_count + 1,
          last_seen_at: now,
          last_error_id: id
        })
        |> Canary.Repo.update!()
    end
  end
end
