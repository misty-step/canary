defmodule Canary.QueryTest do
  use Canary.DataCase

  alias Canary.Query
  alias Canary.Schemas.ErrorGroup

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
end
