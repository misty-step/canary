defmodule CanaryWeb.ApiKeyScopeTest do
  use CanaryWeb.ConnCase

  describe "scope enforcement" do
    test "read-only keys can query but cannot ingest", %{conn: conn} do
      {read_key, _api_key} = create_api_key("reader", "read-only")

      report_conn =
        conn
        |> authenticate(read_key)
        |> get("/api/v1/report?window=1h")

      assert json_response(report_conn, 200)["status"] in [
               "healthy",
               "degraded",
               "down",
               "unknown",
               "empty"
             ]

      ingest_conn =
        build_conn()
        |> authenticate(read_key)
        |> post("/api/v1/errors", %{
          "service" => "billing-api",
          "error_class" => "RuntimeError",
          "message" => "should fail"
        })

      body = json_response(ingest_conn, 403)
      assert body["code"] == "insufficient_scope"
      assert body["scope"] == "read-only"
      assert body["required_scopes"] == ["admin", "ingest-only"]
    end

    test "ingest-only keys can ingest but cannot read reports or manage admin routes", %{
      conn: conn
    } do
      {ingest_key, _api_key} = create_api_key("ingester", "ingest-only")

      ingest_conn =
        conn
        |> authenticate(ingest_key)
        |> post("/api/v1/errors", %{
          "service" => "billing-api",
          "error_class" => "RuntimeError",
          "message" => "works"
        })

      assert json_response(ingest_conn, 201)["id"] =~ ~r/^ERR-/

      report_conn =
        build_conn()
        |> authenticate(ingest_key)
        |> get("/api/v1/report?window=1h")

      report_body = json_response(report_conn, 403)
      assert report_body["code"] == "insufficient_scope"
      assert report_body["required_scopes"] == ["admin", "read-only"]

      admin_conn =
        build_conn()
        |> authenticate(ingest_key)
        |> get("/api/v1/keys")

      admin_body = json_response(admin_conn, 403)
      assert admin_body["code"] == "insufficient_scope"
      assert admin_body["required_scopes"] == ["admin"]
    end
  end
end
