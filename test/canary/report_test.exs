defmodule Canary.ReportTest do
  use Canary.DataCase

  alias Canary.Errors.Ingest
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
      create_monitor_with_state("desktop-active-timer", "up")
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

      assert Enum.any?(result.monitors, fn monitor ->
               monitor.name == "desktop-active-timer" and monitor.state == "up"
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

      assert Enum.map(result.recent_transitions, & &1.name) == ["recent"]
      assert hd(result.recent_transitions).transitioned_at == two_hours_ago
    end

    test "returns invalid_window for unsupported window" do
      assert {:error, :invalid_window} = Report.generate(window: "99h")
    end

    test "includes classification on each error group" do
      {:ok, _} =
        Ingest.ingest(%{
          "service" => "volume",
          "error_class" => "DBConnection.ConnectionError",
          "message" => "database unavailable"
        })

      assert {:ok, result} = Report.generate(window: "1h")
      assert [group] = result.error_groups

      assert group.classification == %{
               category: "infrastructure",
               persistence: "transient",
               component: "database"
             }
    end

    test "returns empty status when no targets or errors exist" do
      assert {:ok, result} = Report.generate(window: "1h")

      assert result.status == "empty"
      assert result.targets == []
      assert result.monitors == []
      assert result.error_groups == []
      assert result.recent_transitions == []
      assert result.summary == "No services configured."
      assert result.truncated == false
      assert result.cursor == nil
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

    test "applies a default limit to error groups but not targets" do
      for name <- ~w(alpha bravo), do: create_target_with_state(name, "up")
      create_monitor_with_state("desktop-active-timer", "up")

      for index <- 1..30 do
        create_error_group("svc-#{index}", "Error#{index}", 100 - index)
      end

      assert {:ok, result} = Report.generate(window: "1h")

      assert length(result.targets) == 2
      assert length(result.monitors) == 1
      assert length(result.error_groups) == 25
      assert result.truncated == true
      assert is_binary(result.cursor)
    end

    test "paginates targets and error groups without duplicates" do
      for name <- ~w(alpha bravo charlie delta echo foxtrot golf) do
        create_target_with_state(name, "up")
      end

      for {service, count} <- Enum.zip(~w(svc-a svc-b svc-c svc-d svc-e svc-f svc-g), 70..64//-1) do
        create_error_group(service, "ConnectionError", count)
      end

      assert {:ok, first_page} = Report.generate(window: "1h", limit: 5)

      assert Enum.map(first_page.targets, & &1.name) == ~w(alpha bravo charlie delta echo)
      assert Enum.map(first_page.error_groups, & &1.service) == ~w(svc-a svc-b svc-c svc-d svc-e)
      assert first_page.truncated == true
      assert is_binary(first_page.cursor)

      assert {:ok, second_page} =
               Report.generate(window: "1h", limit: 5, cursor: first_page.cursor)

      assert Enum.map(second_page.targets, & &1.name) == ~w(foxtrot golf)
      assert Enum.map(second_page.error_groups, & &1.service) == ~w(svc-f svc-g)
      assert second_page.truncated == false
      assert second_page.cursor == nil

      assert MapSet.disjoint?(
               MapSet.new(Enum.map(first_page.targets, & &1.id)),
               MapSet.new(Enum.map(second_page.targets, & &1.id))
             )

      assert MapSet.disjoint?(
               MapSet.new(Enum.map(first_page.error_groups, & &1.group_hash)),
               MapSet.new(Enum.map(second_page.error_groups, & &1.group_hash))
             )
    end

    test "does not replay targets after they are exhausted on an earlier page" do
      create_target_with_state("alpha", "up")

      for index <- 1..8 do
        create_error_group("svc-#{index}", "Error#{index}", 100 - index)
      end

      assert {:ok, first_page} = Report.generate(window: "1h", limit: 5)
      assert Enum.map(first_page.targets, & &1.name) == ["alpha"]
      assert is_binary(first_page.cursor)

      assert {:ok, second_page} =
               Report.generate(window: "1h", limit: 5, cursor: first_page.cursor)

      assert second_page.targets == []
      assert length(second_page.error_groups) == 3
    end
  end

  defp create_error_group_hash(service, error_class) do
    :crypto.hash(:sha256, "#{service}:#{error_class}") |> Base.encode16(case: :lower)
  end
end
