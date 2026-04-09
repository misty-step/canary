defmodule CanaryWeb.MetricsControllerTest do
  use CanaryWeb.ConnCase

  describe "GET /metrics" do
    test "requires auth and returns Prometheus exposition", %{conn: conn} do
      Canary.Metrics.emit_runtime_metrics()
      {raw_key, _key} = create_api_key()

      conn =
        conn
        |> authenticate(raw_key)
        |> get("/metrics")

      body = response(conn, 200)

      assert hd(get_resp_header(conn, "content-type")) =~ "text/plain"
      assert body =~ "# HELP"
      assert body =~ "# TYPE"
      assert body =~ "canary_webhook_queue_depth"
      assert body =~ "canary_oban_queue_depth"
    end

    test "returns 401 without auth", %{conn: conn} do
      conn = get(conn, "/metrics")

      assert get_resp_header(conn, "content-type") == ["application/problem+json; charset=utf-8"]
      assert json_response(conn, 401)["code"] == "invalid_api_key"
    end
  end
end
