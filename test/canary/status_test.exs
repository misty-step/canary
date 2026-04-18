defmodule Canary.StatusTest do
  use Canary.DataCase

  alias Canary.Status
  import Canary.Fixtures

  setup do
    clean_status_tables()
    :ok
  end

  describe "combined/0" do
    test "all healthy targets and no errors" do
      for name <- ["alpha", "bravo", "charlie"], do: create_target_with_state(name, "up")

      assert {:ok, result} = Status.combined()

      assert result.overall == "healthy"
      assert length(result.targets) == 3
      assert result.monitors == []
      assert Enum.all?(result.targets, &(&1.state == "up"))
      assert result.error_summary == []
      assert result.summary =~ "All 3 health surfaces healthy"
    end

    test "target down with errors for that service" do
      create_target_with_state("volume", "down")
      create_target_with_state("api", "up")
      create_error_group("volume", "ConnectionError", 12)

      assert {:ok, result} = Status.combined()

      assert result.overall == "unhealthy"

      unhealthy = Enum.filter(result.targets, &(&1.state != "up"))
      assert length(unhealthy) == 1
      assert hd(unhealthy).name == "volume"

      volume_errors = Enum.find(result.error_summary, &(&1.service == "volume"))
      assert volume_errors.total_count == 12
      assert result.summary =~ "volume"
    end

    test "no targets and no errors" do
      assert {:ok, result} = Status.combined()

      assert result.overall == "empty"
      assert result.targets == []
      assert result.monitors == []
      assert result.error_summary == []
      assert result.summary =~ "No services configured"
    end

    test "degraded target without errors" do
      create_target_with_state("api", "degraded")
      create_target_with_state("web", "up")

      assert {:ok, result} = Status.combined()

      assert result.overall == "degraded"
      assert result.summary =~ "degraded"
    end

    test "errors exist but all targets healthy" do
      create_target_with_state("api", "up")
      create_error_group("api", "TimeoutError", 5)

      assert {:ok, result} = Status.combined()

      assert result.overall == "warning"
      assert result.summary =~ "error"
    end

    test "errors exist with no targets returns warning" do
      create_error_group("orphan-svc", "CrashError", 3)

      assert {:ok, result} = Status.combined()

      assert result.overall == "warning"
      assert result.summary =~ "error"
    end

    test "monitors participate in overall health without pretending to be targets" do
      create_monitor_with_state("desktop-active-timer", "down")

      assert {:ok, result} = Status.combined()

      assert result.overall == "unhealthy"
      assert result.targets == []
      assert [%{name: "desktop-active-timer", state: "down"}] = result.monitors
    end

    test "unknown target state treated as degraded" do
      create_target_with_state("booting", "unknown")

      assert {:ok, result} = Status.combined()

      assert result.overall == "degraded"
    end

    test "old errors outside 1h window are excluded" do
      create_target_with_state("api", "up")

      two_hours_ago =
        DateTime.utc_now()
        |> DateTime.add(-7_200, :second)
        |> DateTime.to_iso8601()

      create_error_group("api", "StaleError", 10, last_seen_at: two_hours_ago)

      assert {:ok, result} = Status.combined()

      assert result.overall == "healthy"
      assert result.error_summary == []
    end

    test "singular service wording with one service" do
      create_target_with_state("api", "up")
      create_error_group("api", "TimeoutError", 5)

      assert {:ok, result} = Status.combined()

      assert result.summary =~ "1 service in the last hour"
    end

    test "returns invalid_window for unsupported window" do
      assert {:error, :invalid_window} = Status.combined("99h")
    end
  end
end
