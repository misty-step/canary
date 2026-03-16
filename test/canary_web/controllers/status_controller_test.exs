defmodule CanaryWeb.StatusControllerTest do
  use CanaryWeb.ConnCase

  import Canary.Fixtures

  setup %{conn: conn} do
    clean_status_tables()
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "GET /api/v1/status" do
    test "all healthy, no errors", %{conn: conn} do
      for name <- ["alpha", "bravo", "charlie"], do: create_target_with_state(name, "up")

      conn = get(conn, "/api/v1/status")
      body = json_response(conn, 200)

      assert body["overall"] == "healthy"
      assert length(body["targets"]) == 3
      assert body["error_summary"] == []
      assert is_binary(body["summary"])
    end

    test "target down with errors", %{conn: conn} do
      create_target_with_state("volume", "down")
      create_error_group("volume", "ConnectionError", 12)

      conn = get(conn, "/api/v1/status")
      body = json_response(conn, 200)

      assert body["overall"] == "unhealthy"
      assert [error_entry] = body["error_summary"]
      assert error_entry["service"] == "volume"
      assert error_entry["total_count"] == 12
    end

    test "no targets and no errors", %{conn: conn} do
      conn = get(conn, "/api/v1/status")
      body = json_response(conn, 200)

      assert body["overall"] == "empty"
      assert body["targets"] == []
      assert body["error_summary"] == []
      assert body["summary"] =~ "No services configured"
    end

    test "401 without auth", %{conn: conn} do
      conn =
        conn
        |> delete_req_header("authorization")
        |> get("/api/v1/status")

      assert json_response(conn, 401)["code"] == "invalid_api_key"
    end
  end
end
