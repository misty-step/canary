defmodule Canary.Errors.IngestTest do
  use Canary.DataCase

  alias Canary.Errors.Ingest
  alias Canary.Schemas.{Error, ErrorGroup}

  @valid_attrs %{
    "service" => "cadence",
    "error_class" => "RuntimeError",
    "message" => "something went wrong"
  }

  describe "ingest/1" do
    test "creates error and group for new error" do
      {:ok, result} = Ingest.ingest(@valid_attrs)

      assert String.starts_with?(result.id, "ERR-")
      assert is_binary(result.group_hash)
      assert result.is_new_class == true

      assert Repo.get(Error, result.id)
      assert Repo.get(ErrorGroup, result.group_hash)
    end

    test "increments group count on duplicate" do
      {:ok, r1} = Ingest.ingest(@valid_attrs)
      {:ok, r2} = Ingest.ingest(@valid_attrs)

      assert r1.group_hash == r2.group_hash
      assert r2.is_new_class == false

      group = Repo.get(ErrorGroup, r1.group_hash)
      assert group.total_count == 2
    end

    test "uses fingerprint for grouping when provided" do
      attrs = Map.put(@valid_attrs, "fingerprint", ["custom-group"])
      {:ok, r1} = Ingest.ingest(attrs)

      attrs2 =
        Map.merge(@valid_attrs, %{
          "message" => "totally different",
          "fingerprint" => ["custom-group"]
        })

      {:ok, r2} = Ingest.ingest(attrs2)

      assert r1.group_hash == r2.group_hash
    end

    test "rejects missing required fields" do
      {:error, :validation_error, errors} = Ingest.ingest(%{"service" => "svc"})
      field_names = Enum.map(errors, fn {name, _} -> name end)
      assert "error_class" in field_names
      assert "message" in field_names
    end

    test "stores severity and environment defaults" do
      {:ok, result} = Ingest.ingest(@valid_attrs)
      error = Repo.get(Error, result.id)

      assert error.severity == "error"
      assert error.environment == "production"
    end

    test "accepts custom severity" do
      attrs = Map.put(@valid_attrs, "severity", "warning")
      {:ok, result} = Ingest.ingest(attrs)
      error = Repo.get(Error, result.id)

      assert error.severity == "warning"
    end

    test "rejects non-string fingerprint elements" do
      attrs = Map.put(@valid_attrs, "fingerprint", ["ok", 123])
      {:error, :validation_error, errors} = Ingest.ingest(attrs)
      assert errors == %{"fingerprint" => ["elements must be strings"]}
    end
  end
end
