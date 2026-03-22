defmodule Canary.QueryTest do
  use Canary.DataCase

  alias Canary.Query
  alias Canary.Schemas.{ErrorGroup, Target, TargetCheck, TargetState}

  defp insert_group!(attrs) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    defaults = %{
      group_hash: Nanoid.generate(),
      service: "test-svc",
      error_class: "RuntimeError",
      severity: "error",
      first_seen_at: now,
      last_seen_at: now,
      last_error_id: "ERR-#{Nanoid.generate()}",
      total_count: 1,
      status: "active"
    }

    %ErrorGroup{}
    |> ErrorGroup.changeset(Map.merge(defaults, attrs))
    |> Canary.Repo.insert!()
  end

  defp insert_target!(attrs \\ %{}) do
    id = Canary.ID.target_id()
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    defaults = %{url: "https://example.com", name: "target-#{id}", created_at: now}

    %Target{id: id}
    |> Target.changeset(Map.merge(defaults, attrs))
    |> Canary.Repo.insert!()
  end

  defp insert_state!(target_id, attrs \\ %{}) do
    defaults = %{state: "up", consecutive_failures: 0, consecutive_successes: 1}

    %TargetState{target_id: target_id}
    |> TargetState.changeset(Map.merge(defaults, attrs))
    |> Canary.Repo.insert!()
  end

  defp insert_check!(target_id, attrs) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()
    defaults = %{target_id: target_id, checked_at: now, result: "success", latency_ms: 100}

    %TargetCheck{}
    |> TargetCheck.changeset(Map.merge(defaults, attrs))
    |> Canary.Repo.insert!()
  end

  describe "health_status/0" do
    test "returns empty targets list when no targets exist" do
      result = Query.health_status()
      assert result.targets == []
      assert is_binary(result.summary)
    end

    test "returns target with state from LEFT JOIN" do
      target = insert_target!(%{name: "api"})
      insert_state!(target.id, %{state: "up", consecutive_failures: 0})

      result = Query.health_status()
      assert [t] = result.targets
      assert t.id == target.id
      assert t.name == "api"
      assert t.state == "up"
      assert t.consecutive_failures == 0
    end

    test "returns unknown state when target has no state record" do
      _target = insert_target!()

      result = Query.health_status()
      assert [t] = result.targets
      assert t.state == "unknown"
      assert t.consecutive_failures == 0
    end

    test "includes recent_checks limited to 5 per target" do
      target = insert_target!()
      insert_state!(target.id)

      # Insert 7 checks with descending timestamps
      _checks =
        for i <- 0..6 do
          checked_at =
            DateTime.utc_now()
            |> DateTime.add(-i * 60, :second)
            |> DateTime.to_iso8601()

          insert_check!(target.id, %{checked_at: checked_at, latency_ms: 100 + i})
        end

      result = Query.health_status()
      assert [t] = result.targets
      assert length(t.recent_checks) == 5
      # Most recent first
      assert hd(t.recent_checks).latency_ms == 100
    end

    test "batches checks across multiple targets" do
      t1 = insert_target!(%{name: "alpha"})
      t2 = insert_target!(%{name: "bravo"})
      insert_state!(t1.id, %{state: "up"})
      insert_state!(t2.id, %{state: "degraded"})

      insert_check!(t1.id, %{latency_ms: 50})
      insert_check!(t2.id, %{latency_ms: 200})

      result = Query.health_status()
      assert length(result.targets) == 2

      alpha = Enum.find(result.targets, &(&1.name == "alpha"))
      bravo = Enum.find(result.targets, &(&1.name == "bravo"))

      assert alpha.state == "up"
      assert bravo.state == "degraded"
      assert length(alpha.recent_checks) == 1
      assert length(bravo.recent_checks) == 1
    end

    test "includes tls_expires_at and latency_ms from recent checks" do
      target = insert_target!()
      insert_state!(target.id)

      insert_check!(target.id, %{
        latency_ms: 42,
        tls_expires_at: "2026-12-01T00:00:00Z"
      })

      result = Query.health_status()
      assert [t] = result.targets
      assert t.latency_ms == 42
      assert t.tls_expires_at == "2026-12-01T00:00:00Z"
    end
  end

  describe "errors_by_error_class/3" do
    test "returns errors matching class across multiple services" do
      insert_group!(%{service: "volume", error_class: "RuntimeError"})
      insert_group!(%{service: "canary-triage", error_class: "RuntimeError"})
      insert_group!(%{service: "volume", error_class: "OtherError"})

      assert {:ok, result} = Query.errors_by_error_class("RuntimeError", "24h")
      assert result.error_class == "RuntimeError"
      assert result.total_errors == 2
      assert length(result.groups) == 2

      services = Enum.map(result.groups, & &1.service)
      assert "volume" in services
      assert "canary-triage" in services
    end

    test "returns empty groups when no errors match class" do
      insert_group!(%{error_class: "RuntimeError"})

      assert {:ok, result} = Query.errors_by_error_class("FooError", "24h")
      assert result.total_errors == 0
      assert result.groups == []
    end

    test "filters by both error_class and service when service given" do
      insert_group!(%{service: "volume", error_class: "RuntimeError"})
      insert_group!(%{service: "canary-triage", error_class: "RuntimeError"})

      assert {:ok, result} = Query.errors_by_error_class("RuntimeError", "24h", service: "volume")
      assert result.total_errors == 1
      assert [group] = result.groups
      assert group.service == "volume"
    end

    test "returns invalid_window error for bad window" do
      assert {:error, :invalid_window} = Query.errors_by_error_class("RuntimeError", "99h")
    end
  end

  describe "error_groups/1" do
    test "orders groups by count, then service, then error class" do
      insert_group!(%{service: "beta", error_class: "ZedError", total_count: 3})
      insert_group!(%{service: "alpha", error_class: "AlphaError", total_count: 5})
      insert_group!(%{service: "alpha", error_class: "BetaError", total_count: 3})

      assert {:ok, groups} = Query.error_groups("24h")

      assert Enum.map(groups, &{&1.count, &1.service, &1.error_class}) == [
               {5, "alpha", "AlphaError"},
               {3, "alpha", "BetaError"},
               {3, "beta", "ZedError"}
             ]
    end
  end
end
