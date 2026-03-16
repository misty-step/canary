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

  describe "combined_status/3" do
    test "empty — no targets, no errors" do
      assert Summary.combined_status("empty", [], []) == "No services configured."
    end

    test "healthy — all targets up" do
      targets = [%{name: "a", state: "up"}, %{name: "b", state: "up"}]

      assert Summary.combined_status("healthy", targets, []) ==
               "All 2 targets healthy. No errors in the last hour."
    end

    test "unhealthy — multiple down targets" do
      targets = [
        %{name: "api", state: "down"},
        %{name: "web", state: "down"},
        %{name: "db", state: "up"}
      ]

      result = Summary.combined_status("unhealthy", targets, [])

      assert result =~ "3 targets monitored."
      assert result =~ "2 down (api, web)."
      refute result =~ "error"
    end

    test "degraded — mixed degraded states" do
      targets = [
        %{name: "api", state: "degraded"},
        %{name: "web", state: "degraded"},
        %{name: "db", state: "up"}
      ]

      result = Summary.combined_status("degraded", targets, [])

      assert result =~ "2 degraded (api, web)."
    end

    test "mixed down and degraded with errors" do
      targets = [
        %{name: "api", state: "down"},
        %{name: "web", state: "degraded"}
      ]

      errors = [%{service: "api", total_count: 10, unique_classes: 2}]
      result = Summary.combined_status("unhealthy", targets, errors)

      assert result =~ "1 down (api)."
      assert result =~ "1 degraded (web)."
      assert result =~ "10 errors across 1 service in the last hour"
    end

    test "warning — errors only, no targets" do
      errors = [%{service: "orphan", total_count: 5, unique_classes: 1}]
      result = Summary.combined_status("warning", [], errors)

      assert result =~ "0 targets monitored."
      assert result =~ "5 errors"
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
