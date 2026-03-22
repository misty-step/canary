defmodule Canary.ReportTest do
  use Canary.DataCase

  alias Canary.Report
  alias Canary.Schemas.{Target, TargetState}
  alias Canary.Incidents
  import Canary.Fixtures

  setup do
    clean_status_tables()
    :ok
  end

  describe "generate/1" do
    test "includes health and error signals for the same service" do
      create_target_with_state("volume", "degraded")
      create_target_with_state("api", "up")
      create_error_group("volume", "ConnectionError", 12)
      {:ok, _incident} = Incidents.correlate(:health_transition, "TGT-volume", "volume")

      {:ok, _incident} =
        Incidents.correlate(
          :error_group,
          create_error_group_hash("volume", "ConnectionError"),
          "volume"
        )

      assert {:ok, result} = Report.generate(window: "1h")

      assert result.status == "degraded"

      assert Enum.any?(result.targets, fn target ->
               target.name == "volume" and target.state == "degraded"
             end)

      assert Enum.any?(result.error_groups, fn group ->
               group.service == "volume" and group.error_class == "ConnectionError"
             end)

      assert [%{service: "volume", signal_count: 2}] = result.incidents
      assert is_binary(result.summary)
    end

    test "scopes error groups and recent transitions to the requested window" do
      create_target_with_state("recent", "degraded")
      create_target_with_state("stale", "down")

      now = DateTime.utc_now()
      two_hours_ago = now |> DateTime.add(-2 * 3_600, :second) |> DateTime.to_iso8601()
      eight_hours_ago = now |> DateTime.add(-8 * 3_600, :second) |> DateTime.to_iso8601()

      Canary.Repo.update_all(
        from(s in TargetState, where: s.target_id == "TGT-recent"),
        set: [last_transition_at: two_hours_ago]
      )

      Canary.Repo.update_all(
        from(s in TargetState, where: s.target_id == "TGT-stale"),
        set: [last_transition_at: eight_hours_ago]
      )

      create_error_group("recent", "RecentError", 3, last_seen_at: two_hours_ago)
      create_error_group("stale", "StaleError", 4, last_seen_at: eight_hours_ago)

      assert {:ok, result} = Report.generate(window: "6h")

      assert Enum.map(result.error_groups, & &1.service) == ["recent"]
      assert result.incidents == []

      assert Enum.map(result.recent_transitions, & &1.target_name) == ["recent"]
      assert hd(result.recent_transitions).transitioned_at == two_hours_ago
    end

    test "returns invalid_window for unsupported window" do
      assert {:error, :invalid_window} = Report.generate(window: "99h")
    end

    test "returns empty status when no targets or errors exist" do
      assert {:ok, result} = Report.generate(window: "1h")

      assert result.status == "empty"
      assert result.targets == []
      assert result.error_groups == []
      assert result.recent_transitions == []
      assert result.summary == "No services configured."
    end

    test "reports unknown state for targets without a state record" do
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      Canary.Repo.insert!(%Target{
        id: "TGT-orphan",
        name: "orphan",
        url: "https://orphan.example.com/healthz",
        created_at: now
      })

      assert {:ok, result} = Report.generate(window: "1h")
      assert [%{name: "orphan", state: "unknown"}] = result.targets
    end
  end

  defp create_error_group_hash(service, error_class) do
    :crypto.hash(:sha256, "#{service}:#{error_class}") |> Base.encode16(case: :lower)
  end
end
