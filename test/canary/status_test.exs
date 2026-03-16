defmodule Canary.StatusTest do
  use Canary.DataCase

  alias Canary.Status
  import Canary.Fixtures

  setup do
    Canary.Repo.delete_all(Canary.Schemas.TargetState)
    Canary.Repo.delete_all(Canary.Schemas.TargetCheck)
    Canary.Repo.delete_all(Canary.Schemas.Target)
    Canary.Repo.delete_all(Canary.Schemas.ErrorGroup)
    :ok
  end

  describe "combined/0" do
    test "all healthy targets and no errors" do
      for name <- ["alpha", "bravo", "charlie"], do: create_target_with_state(name, "up")

      result = Status.combined()

      assert result.overall == "healthy"
      assert length(result.targets) == 3
      assert Enum.all?(result.targets, &(&1.state == "up"))
      assert result.error_summary == []
      assert result.summary =~ "All 3 targets healthy"
    end

    test "target down with errors for that service" do
      create_target_with_state("volume", "down")
      create_target_with_state("api", "up")
      create_error_group("volume", "ConnectionError", 12)

      result = Status.combined()

      assert result.overall == "unhealthy"

      unhealthy = Enum.filter(result.targets, &(&1.state != "up"))
      assert length(unhealthy) == 1
      assert hd(unhealthy).name == "volume"

      volume_errors = Enum.find(result.error_summary, &(&1.service == "volume"))
      assert volume_errors.total_count == 12
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
      create_error_group("api", "TimeoutError", 5)

      result = Status.combined()

      assert result.overall == "warning"
      assert result.summary =~ "error"
    end

    test "errors exist with no targets returns warning" do
      create_error_group("orphan-svc", "CrashError", 3)

      result = Status.combined()

      assert result.overall == "warning"
      assert result.summary =~ "error"
    end

    test "unknown target state treated as degraded" do
      create_target_with_state("booting", "unknown")

      result = Status.combined()

      assert result.overall == "degraded"
    end
  end
end
