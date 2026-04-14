defmodule Canary.QueryErrorsByServiceOptsTest do
  use Canary.DataCase

  import Canary.Fixtures

  alias Canary.Query
  alias Canary.Schemas.ErrorGroup

  setup do
    clean_status_tables()
    :ok
  end

  describe "errors_by_service opts consolidation" do
    test "keyword opts work as third argument (no separate cursor param)" do
      group = create_error_group("svc-opts", "RuntimeError", 3)
      create_annotation(:group, group.group_hash, action: "acknowledged")

      # This should work: passing opts as the 3rd arg with keyword list
      {:ok, result} =
        Query.errors_by_service("svc-opts", "24h", without_annotation: "acknowledged")

      hashes = Enum.map(result.groups, & &1.group_hash)
      refute group.group_hash in hashes
    end

    test "cursor can be passed inside opts keyword" do
      _group = create_error_group("svc-cursor", "RuntimeError", 3)

      {:ok, result} =
        Query.errors_by_service("svc-cursor", "24h", cursor: nil)

      assert is_list(result.groups)
    end

    test "cursor pagination follows the count ordering without duplicates" do
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      for rank <- 1..51 do
        inverse_hash = String.pad_leading(Integer.to_string(52 - rank), 3, "0")
        group_hash = "group-#{inverse_hash}"

        %ErrorGroup{group_hash: group_hash}
        |> ErrorGroup.changeset(%{
          service: "svc-page",
          error_class: "RuntimeError#{rank}",
          severity: "error",
          first_seen_at: now,
          last_seen_at: now,
          last_error_id: "ERR-page-#{rank}",
          total_count: 200 - rank,
          status: "active"
        })
        |> Repo.insert!()
      end

      {:ok, first_page} = Query.errors_by_service("svc-page", "24h")
      assert length(first_page.groups) == 50
      assert is_binary(first_page.cursor)

      first_hashes = Enum.map(first_page.groups, & &1.group_hash)

      {:ok, second_page} =
        Query.errors_by_service("svc-page", "24h", cursor: first_page.cursor)

      assert Enum.map(second_page.groups, & &1.group_hash) == ["group-001"]
      assert second_page.cursor == nil
      refute Enum.any?(second_page.groups, &(&1.group_hash in first_hashes))
    end
  end
end
