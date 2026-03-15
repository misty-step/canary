defmodule CanaryWeb.HealthControllerTest do
  use CanaryWeb.ConnCase

  describe "GET /healthz" do
    test "200 with no auth", %{conn: conn} do
      conn = get(conn, "/healthz")
      assert json_response(conn, 200) == %{"status" => "ok"}
    end
  end

  describe "GET /readyz" do
    test "200 when DB is healthy with no auth", %{conn: conn} do
      conn = get(conn, "/readyz")

      body = json_response(conn, 200)
      assert body["status"] == "ready"
      assert body["checks"]["database"] == "ok"
    end
  end

  describe "GET /api/v1/health-status" do
    test "200 with summary", %{conn: conn} do
      {raw_key, _key} = create_api_key()

      conn =
        conn
        |> authenticate(raw_key)
        |> get("/api/v1/health-status")

      body = json_response(conn, 200)
      assert is_binary(body["summary"])
      assert is_list(body["targets"])
    end

    test "401 without auth", %{conn: conn} do
      conn = get(conn, "/api/v1/health-status")
      assert json_response(conn, 401)["code"] == "invalid_api_key"
    end
  end
end
