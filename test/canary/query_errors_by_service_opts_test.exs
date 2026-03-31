defmodule Canary.QueryErrorsByServiceOptsTest do
  use Canary.DataCase

  import Canary.Fixtures

  alias Canary.Query

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
  end
end
