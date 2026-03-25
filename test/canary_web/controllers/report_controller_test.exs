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
               "cursor",
               "error_groups",
               "incidents",
               "recent_transitions",
               "status",
               "summary",
               "targets",
               "truncated"
             ]

      assert body["status"] == "degraded"
      assert [%{"service" => "volume"}] = body["error_groups"]
      assert [%{"service" => "volume", "signal_count" => 2}] = body["incidents"]
      assert is_list(body["targets"])
      assert is_list(body["recent_transitions"])
      assert body["truncated"] == false
      assert is_nil(body["cursor"])
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
      assert body["truncated"] == false
      assert is_nil(body["cursor"])
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

    test "applies limit and cursor pagination to targets and error groups", %{conn: conn} do
      for name <- ~w(alpha bravo charlie delta echo foxtrot golf) do
        create_target_with_state(name, "up")
      end

      for {service, count} <- Enum.zip(~w(svc-a svc-b svc-c svc-d svc-e svc-f svc-g), 70..64//-1) do
        create_error_group(service, "ConnectionError", count)
      end

      first_conn = get(conn, "/api/v1/report?limit=5")
      first_body = json_response(first_conn, 200)

      assert first_body["truncated"] == true
      assert is_binary(first_body["cursor"])
      assert Enum.map(first_body["targets"], & &1["name"]) == ~w(alpha bravo charlie delta echo)

      assert Enum.map(first_body["error_groups"], & &1["service"]) ==
               ~w(svc-a svc-b svc-c svc-d svc-e)

      second_conn = get(conn, "/api/v1/report?limit=5&cursor=#{first_body["cursor"]}")
      second_body = json_response(second_conn, 200)

      assert second_body["truncated"] == false
      assert is_nil(second_body["cursor"])
      assert Enum.map(second_body["targets"], & &1["name"]) == ~w(foxtrot golf)
      assert Enum.map(second_body["error_groups"], & &1["service"]) == ~w(svc-f svc-g)
    end

    test "returns csv when requested", %{conn: conn} do
      create_target_with_state("alpha", "degraded")
      create_error_group("volume", "ConnectionError", 12)

      conn =
        conn
        |> put_req_header("accept", "text/csv")
        |> get("/api/v1/report?limit=5")

      assert get_resp_header(conn, "content-type") == ["text/csv; charset=utf-8"]
      assert response(conn, 200) =~ "section,position,id,name,service,error_class,url,state,count"
      assert response(conn, 200) =~ "targets,1,TGT-alpha,alpha"
      assert response(conn, 200) =~ "error_groups,1,"
    end
  end

  defp group_hash(service, error_class) do
    :crypto.hash(:sha256, "#{service}:#{error_class}") |> Base.encode16(case: :lower)
  end
end
