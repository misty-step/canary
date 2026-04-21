defmodule Canary.Query.ErrorsTest do
  use Canary.DataCase

  alias Canary.Query.Errors
  alias Canary.Schemas.{Error, ErrorGroup}

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

  defp insert_error!(attrs) do
    id = Canary.ID.error_id()
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    defaults = %{
      service: "test-svc",
      error_class: "RuntimeError",
      message: "boom",
      group_hash: "grp-#{id}",
      created_at: now
    }

    %Error{id: id}
    |> Error.changeset(Map.merge(defaults, attrs))
    |> Canary.Repo.insert!()
  end

  describe "errors_by_class/1 summary" do
    test "returns non-empty summary with populated groups" do
      insert_group!(%{error_class: "RuntimeError", total_count: 7})
      insert_group!(%{error_class: "ArgumentError", total_count: 3})

      assert {:ok, result} = Errors.errors_by_class("24h")
      assert is_binary(result.summary)
      assert byte_size(result.summary) > 0
      assert result.total_errors == 10
      assert result.window == "24h"
      assert length(result.groups) == 2
      assert result.summary =~ "10 errors"
      assert result.summary =~ "Most frequent: RuntimeError"
    end

    test "returns non-empty summary with no groups" do
      assert {:ok, result} = Errors.errors_by_class("24h")
      assert is_binary(result.summary)
      assert byte_size(result.summary) > 0
      assert result.total_errors == 0
      assert result.groups == []
    end

    test "returns invalid_window for bad window" do
      assert {:error, :invalid_window} = Errors.errors_by_class("99h")
    end
  end

  describe "map-returning query contract" do
    test "every map-returning endpoint includes a non-empty :summary" do
      error =
        insert_error!(%{
          service: "volume",
          error_class: "RuntimeError",
          group_hash: "grp-contract-1"
        })

      insert_group!(%{
        service: "volume",
        error_class: "RuntimeError",
        group_hash: "grp-contract-1",
        last_error_id: error.id,
        total_count: 4
      })

      responses = [
        {:errors_by_service, Errors.errors_by_service("volume", "1h")},
        {:errors_by_error_class, Errors.errors_by_error_class("RuntimeError", "24h")},
        {:errors_by_class, Errors.errors_by_class("24h")},
        {:error_detail, Errors.error_detail(error.id)}
      ]

      for {name, {:ok, result}} <- responses do
        assert is_map(result), "expected #{name} to return a map"

        assert is_binary(result[:summary]),
               "expected #{name}'s response to include a :summary string, got: #{inspect(result)}"

        assert byte_size(result[:summary]) > 0,
               "expected #{name}'s :summary to be non-empty"
      end
    end
  end
end
