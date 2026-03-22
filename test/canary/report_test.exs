defmodule Canary.ReportTest do
  use Canary.DataCase

  alias Canary.Report
  alias Canary.Schemas.TargetState
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

      assert {:ok, result} = Report.generate(window: "1h")

      assert result.status == "degraded"

      assert Enum.any?(result.targets, fn target ->
               target.name == "volume" and target.state == "degraded"
             end)

      assert Enum.any?(result.error_groups, fn group ->
               group.service == "volume" and group.error_class == "ConnectionError"
             end)

      assert is_binary(result.summary)
    end

    test "scopes error groups and recent transitions to the requested window" do
      create_target_with_state("recent", "degraded")
      create_target_with_state("stale", "down")

      now = DateTime.utc_now()
      two_hours_ago = now |> DateTime.add(-7_200, :second) |> DateTime.to_iso8601()
      eight_hours_ago = now |> DateTime.add(-28_800, :second) |> DateTime.to_iso8601()

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

      assert Enum.map(result.recent_transitions, & &1.target_name) == ["recent"]
      assert hd(result.recent_transitions).transitioned_at == two_hours_ago
    end
  end
end
