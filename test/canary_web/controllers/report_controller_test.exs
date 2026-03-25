defmodule CanaryWeb.ReportControllerTest do
  use CanaryWeb.ConnCase

  import Canary.Fixtures
  alias Canary.Incidents
  alias Canary.Schemas.{Error, ErrorGroup}

  setup %{conn: conn} do
    clean_status_tables()
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "GET /api/v1/report" do
    test "returns the unified report payload", %{conn: conn} do
      create_target_with_state("volume", "degraded")
      create_error_group("volume", "ConnectionError", 12)
      {:ok, _incident} = Incidents.correlate(:health_transition, "TGT-volume", "volume")

      {:ok, _incident} =
        Incidents.correlate(:error_group, group_hash("volume", "ConnectionError"), "volume")

      conn = get(conn, "/api/v1/report?window=1h")
      body = json_response(conn, 200)

      assert Enum.sort(Map.keys(body)) == [
               "error_groups",
               "incidents",
               "recent_transitions",
               "status",
               "summary",
               "targets"
             ]

      assert body["status"] == "degraded"
      assert [%{"service" => "volume"}] = body["error_groups"]
      assert [%{"service" => "volume", "signal_count" => 2}] = body["incidents"]
      assert is_list(body["targets"])
      assert is_list(body["recent_transitions"])
    end

    test "includes classification for each error group", %{conn: conn} do
      post(conn, "/api/v1/errors", %{
        "service" => "volume",
        "error_class" => "DBConnection.ConnectionError",
        "message" => "database unavailable"
      })

      conn = get(conn, "/api/v1/report?window=1h")
      body = json_response(conn, 200)

      assert [%{"classification" => classification}] = body["error_groups"]

      assert classification == %{
               "category" => "infrastructure",
               "persistence" => "transient",
               "component" => "database"
             }
    end

    test "returns healthy status and no error groups when everything is healthy", %{conn: conn} do
      for name <- ["alpha", "bravo"], do: create_target_with_state(name, "up")

      conn = get(conn, "/api/v1/report")
      body = json_response(conn, 200)

      assert body["status"] == "healthy"
      assert body["error_groups"] == []
      assert body["incidents"] == []
      assert length(body["targets"]) == 2
      assert is_binary(body["summary"])
    end

    test "includes search_results when q is provided", %{conn: conn} do
      {:ok, _result} =
        Canary.Errors.Ingest.ingest(%{
          "service" => "volume",
          "error_class" => "TimeoutError",
          "message" => "timeout while reporting health"
        })

      conn = get(conn, "/api/v1/report?q=timeout")
      body = json_response(conn, 200)

      assert [%{"service" => "volume", "message" => "timeout while reporting health"}] =
               body["search_results"]
    end

    test "scopes search_results to the requested window", %{conn: conn} do
      two_hours_ago =
        DateTime.utc_now()
        |> DateTime.add(-7_200, :second)
        |> DateTime.to_iso8601()

      {:ok, result} =
        Canary.Errors.Ingest.ingest(%{
          "service" => "volume",
          "error_class" => "TimeoutError",
          "message" => "timeout from two hours ago"
        })

      Canary.Repo.get!(Error, result.id)
      |> Ecto.Changeset.change(created_at: two_hours_ago)
      |> Canary.Repo.update!()

      Canary.Repo.get!(ErrorGroup, result.group_hash)
      |> Ecto.Changeset.change(first_seen_at: two_hours_ago, last_seen_at: two_hours_ago)
      |> Canary.Repo.update!()

      conn = get(conn, "/api/v1/report?window=1h&q=timeout")
      body = json_response(conn, 200)

      assert body["error_groups"] == []
      assert body["search_results"] == []
    end

    test "returns 422 for an invalid q shape", %{conn: conn} do
      conn = get(conn, "/api/v1/report", %{"q" => ["timeout"]})
      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["errors"]["q"] == ["must be a string"]
    end

    test "returns 401 without auth", %{conn: conn} do
      conn =
        conn
        |> delete_req_header("authorization")
        |> get("/api/v1/report")

      assert json_response(conn, 401)["code"] == "invalid_api_key"
    end

    test "returns 422 for an invalid window", %{conn: conn} do
      conn = get(conn, "/api/v1/report?window=99h")
      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["errors"]["window"] == ["must be one of: 1h, 6h, 24h, 7d, 30d"]
    end
  end

  defp group_hash(service, error_class) do
    :crypto.hash(:sha256, "#{service}:#{error_class}") |> Base.encode16(case: :lower)
  end
end
