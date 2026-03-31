defmodule Canary.QueryAnnotationTest do
  use Canary.DataCase

  import Canary.Fixtures

  alias Canary.Query

  setup do
    clean_status_tables()
    :ok
  end

  describe "errors_by_service with annotation filters" do
    test "without_annotation excludes groups that have the annotation" do
      group = create_error_group("alpha", "RuntimeError", 3)
      create_annotation(:group, group.group_hash, action: "acknowledged")

      {:ok, result} =
        Query.errors_by_service("alpha", "24h", without_annotation: "acknowledged")

      hashes = Enum.map(result.groups, & &1.group_hash)
      refute group.group_hash in hashes
    end

    test "without_annotation includes groups that lack the annotation" do
      group = create_error_group("alpha", "RuntimeError", 3)

      {:ok, result} =
        Query.errors_by_service("alpha", "24h", without_annotation: "acknowledged")

      hashes = Enum.map(result.groups, & &1.group_hash)
      assert group.group_hash in hashes
    end

    test "with_annotation includes groups that have the annotation" do
      group = create_error_group("alpha", "RuntimeError", 3)
      create_annotation(:group, group.group_hash, action: "acknowledged")

      {:ok, result} =
        Query.errors_by_service("alpha", "24h", with_annotation: "acknowledged")

      hashes = Enum.map(result.groups, & &1.group_hash)
      assert group.group_hash in hashes
    end

    test "with_annotation excludes groups that lack the annotation" do
      _group = create_error_group("alpha", "RuntimeError", 3)

      {:ok, result} =
        Query.errors_by_service("alpha", "24h", with_annotation: "acknowledged")

      assert result.groups == []
    end
  end

  describe "errors_by_error_class with annotation filters" do
    test "without_annotation excludes annotated groups" do
      group = create_error_group("alpha", "RuntimeError", 3)
      create_annotation(:group, group.group_hash, action: "triaged")

      {:ok, result} =
        Query.errors_by_error_class("RuntimeError", "24h", without_annotation: "triaged")

      hashes = Enum.map(result.groups, & &1.group_hash)
      refute group.group_hash in hashes
    end

    test "with_annotation includes annotated groups" do
      group = create_error_group("alpha", "RuntimeError", 3)
      create_annotation(:group, group.group_hash, action: "triaged")

      {:ok, result} =
        Query.errors_by_error_class("RuntimeError", "24h", with_annotation: "triaged")

      hashes = Enum.map(result.groups, & &1.group_hash)
      assert group.group_hash in hashes
    end
  end
end
