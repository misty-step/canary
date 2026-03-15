defmodule Canary.Errors.GroupingTest do
  use ExUnit.Case, async: true

  alias Canary.Errors.Grouping

  describe "strip_template/1" do
    test "replaces UUIDs" do
      input = "user 4a8f9c1d-1111-2222-3333-abcdefabcdef not found"
      assert Grouping.strip_template(input) =~ "<uuid>"
      refute Grouping.strip_template(input) =~ "4a8f9c1d"
    end

    test "replaces ISO 8601 timestamps" do
      input = "failed at 2026-03-14T18:00:00Z"
      assert Grouping.strip_template(input) =~ "<timestamp>"
    end

    test "replaces email addresses" do
      input = "user alice@example.com failed"
      assert Grouping.strip_template(input) =~ "<email>"
    end

    test "replaces long hex strings" do
      input = "hash 0xabcdef123456789 invalid"
      assert Grouping.strip_template(input) =~ "<hex>"
    end

    test "replaces 4+ digit integers" do
      input = "user 123456 failed"
      assert Grouping.strip_template(input) =~ "<int>"
    end

    test "collapses whitespace" do
      input = "too   much   space"
      assert Grouping.strip_template(input) == "too much space"
    end

    test "full example from spec" do
      input =
        "user 123456 failed at <path> on 2026-03-14T18:00:00Z request 4a8f9c1d-1111-2222-3333-abcdefabcdef"

      result = Grouping.strip_template(input)
      assert result =~ "<int>"
      assert result =~ "<timestamp>"
      assert result =~ "<uuid>"
    end

    test "preserves short strings" do
      assert Grouping.strip_template("simple error") == "simple error"
    end
  end

  describe "compute_group_hash/1" do
    test "uses fingerprint when provided" do
      attrs1 = %{
        "service" => "svc",
        "error_class" => "E",
        "message" => "m",
        "fingerprint" => ["a", "b"]
      }

      attrs2 = %{
        "service" => "svc",
        "error_class" => "E",
        "message" => "different",
        "fingerprint" => ["a", "b"]
      }

      {hash1, _} = Grouping.compute_group_hash(attrs1)
      {hash2, _} = Grouping.compute_group_hash(attrs2)

      assert hash1 == hash2
    end

    test "different fingerprints produce different hashes" do
      attrs1 = %{
        "service" => "svc",
        "error_class" => "E",
        "message" => "m",
        "fingerprint" => ["a"]
      }

      attrs2 = %{
        "service" => "svc",
        "error_class" => "E",
        "message" => "m",
        "fingerprint" => ["b"]
      }

      {hash1, _} = Grouping.compute_group_hash(attrs1)
      {hash2, _} = Grouping.compute_group_hash(attrs2)

      refute hash1 == hash2
    end

    test "falls back to message template" do
      attrs = %{"service" => "svc", "error_class" => "E", "message" => "user 12345 failed"}
      {hash, template} = Grouping.compute_group_hash(attrs)

      assert is_binary(hash)
      assert String.length(hash) == 64
      assert template =~ "<int>"
    end

    test "same error class + template = same hash" do
      attrs1 = %{"service" => "svc", "error_class" => "E", "message" => "user 11111 failed"}
      attrs2 = %{"service" => "svc", "error_class" => "E", "message" => "user 22222 failed"}

      {hash1, _} = Grouping.compute_group_hash(attrs1)
      {hash2, _} = Grouping.compute_group_hash(attrs2)

      assert hash1 == hash2
    end
  end
end
