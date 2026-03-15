defmodule Canary.SummaryTest do
  use ExUnit.Case, async: true

  alias Canary.Summary

  describe "error_query/1" do
    test "generates summary with groups" do
      result =
        Summary.error_query(%{
          total: 5,
          service: "cadence",
          window: "1h",
          groups: [
            %{error_class: "Ecto.NoResultsError", total_count: 3},
            %{error_class: "RuntimeError", total_count: 2}
          ]
        })

      assert result =~ "5 errors in cadence"
      assert result =~ "1h"
      assert result =~ "2 unique classes"
      assert result =~ "Most frequent: Ecto.NoResultsError (3 occurrences)"
    end

    test "handles empty groups" do
      result =
        Summary.error_query(%{
          total: 0,
          service: "cadence",
          window: "1h",
          groups: []
        })

      assert result =~ "0 errors"
      assert result =~ "0 unique classes"
    end
  end

  describe "health_status/1" do
    test "generates summary with mixed states" do
      result =
        Summary.health_status(%{
          targets: [
            %{name: "api", state: "up"},
            %{name: "web", state: "up"},
            %{name: "db", state: "degraded"},
            %{name: "cache", state: "down"}
          ]
        })

      assert result =~ "4 targets monitored"
      assert result =~ "2 up"
      assert result =~ "1 degraded (db)"
      assert result =~ "1 down (cache)"
    end

    test "all up" do
      result =
        Summary.health_status(%{
          targets: [
            %{name: "api", state: "up"},
            %{name: "web", state: "up"}
          ]
        })

      assert result =~ "2 targets monitored. 2 up."
      refute result =~ "degraded"
      refute result =~ "down"
    end
  end

  describe "error_detail/1" do
    test "generates detail summary" do
      result =
        Summary.error_detail(%{
          error_class: "RuntimeError",
          service: "cadence",
          count: 42,
          first_seen: "2026-03-14T10:00:00Z",
          last_seen: "2026-03-14T18:00:00Z"
        })

      assert result =~ "RuntimeError in cadence"
      assert result =~ "42 times"
      assert result =~ "2026-03-14T10:00:00Z"
    end
  end
end
