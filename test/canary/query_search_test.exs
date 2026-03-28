defmodule Canary.QuerySearchTest do
  use Canary.DataCase

  alias Canary.Errors.Ingest
  alias Canary.Query
  alias Canary.Schemas.Error

  defp ingest_error!(attrs) do
    defaults = %{
      "service" => "canary",
      "error_class" => "RuntimeError",
      "message" => "request timeout while calling upstream",
      "stack_trace" =>
        "RuntimeError: boom\n    lib/canary/query.ex:1: Canary.Query.error_detail/1"
    }

    {:ok, result} = Ingest.ingest(Map.merge(defaults, attrs))
    result
  end

  describe "search/2" do
    test "returns matching errors ranked by relevance" do
      ingest_error!(%{
        "message" => "timeout timeout while calling upstream service",
        "stack_trace" => "RuntimeError: timeout timeout"
      })

      ingest_error!(%{"message" => "timeout while calling upstream service"})
      ingest_error!(%{"message" => "database connection dropped"})

      assert {:ok, results} = Query.search("timeout")
      assert length(results) == 2

      assert [%{message: first_message}, %{message: second_message}] = results
      assert String.contains?(first_message, "timeout timeout")
      assert String.contains?(second_message, "timeout while")
    end

    test "filters matches by service" do
      ingest_error!(%{
        "service" => "linejam",
        "message" => "connection refused when posting issue"
      })

      ingest_error!(%{
        "service" => "canary-obs",
        "message" => "connection refused while calling webhook"
      })

      assert {:ok, [result]} = Query.search("connection refused", service: "linejam")
      assert result.service == "linejam"
    end

    test "indexes newly ingested errors immediately" do
      {:ok, summary} =
        Ingest.ingest(%{
          "service" => "linejam",
          "error_class" => "TimeoutError",
          "message" => "searchable immediately after ingest"
        })

      assert {:ok, [result]} = Query.search("searchable")
      assert result.id == summary.id
    end

    test "returns an empty list when nothing matches" do
      ingest_error!(%{"message" => "database connection dropped"})

      assert {:ok, []} = Query.search("nonexistent")
    end

    test "returns an empty list for a blank query" do
      ingest_error!(%{"message" => "timeout while calling upstream service"})

      assert {:ok, []} = Query.search("   ")
    end

    test "updates search results when an error changes" do
      %{id: error_id} = ingest_error!(%{"message" => "stale timeout message"})
      error = Repo.get!(Error, error_id)

      error
      |> Ecto.Changeset.change(message: "fresh connection failure")
      |> Repo.update!()

      assert {:ok, []} = Query.search("timeout")

      assert {:ok, [%{id: ^error_id, message: "fresh connection failure"}]} =
               Query.search("connection")
    end

    test "removes search results when an error is deleted" do
      %{id: error_id} = ingest_error!(%{"message" => "timeout before delete"})
      error = Repo.get!(Error, error_id)

      assert {:ok, [%{id: ^error_id}]} = Query.search("delete")

      Repo.delete!(error)

      assert {:ok, []} = Query.search("delete")
    end
  end
end
