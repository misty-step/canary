defmodule CanaryWeb.ServiceOnboardingControllerTest do
  use CanaryWeb.ConnCase

  import Canary.Fixtures

  alias Canary.Repo
  alias Canary.Schemas.{ApiKey, Target}

  setup %{conn: conn} do
    clean_status_tables()
    Repo.delete_all(ApiKey)

    {admin_raw_key, _key} = create_api_key("admin")
    conn = authenticate(conn, admin_raw_key)
    {:ok, conn: conn, admin_raw_key: admin_raw_key}
  end

  describe "POST /api/v1/service-onboarding" do
    test "connects a service and makes it visible via the query API", %{
      conn: conn,
      admin_raw_key: admin_raw_key
    } do
      conn =
        post(conn, "/api/v1/service-onboarding", %{
          "service" => "billing-api",
          "url" => "https://example.com/billing/health",
          "environment" => "staging",
          "interval_ms" => 30_000
        })

      body = json_response(conn, 201)

      assert body["service"] == "billing-api"
      assert body["api_key"]["name"] == "billing-api-ingest"
      assert body["api_key"]["scope"] == "ingest-only"
      assert body["target"]["name"] == "billing-api"
      assert body["target"]["service"] == "billing-api"
      assert body["target"]["interval_ms"] == 30_000
      refute Map.has_key?(body["links"], "dashboard")
      assert body["links"]["report"] =~ "/api/v1/report?window=1h"
      assert body["links"]["service_query"] =~ "service=billing-api&window=1h"

      assert body["snippets"]["error_ingest_curl"] =~
               "Authorization: Bearer #{body["api_key"]["key"]}"

      assert body["snippets"]["error_ingest_curl"] =~ "\"service\":\"billing-api\""
      assert body["snippets"]["error_ingest_curl"] =~ "\"environment\":\"staging\""
      assert body["snippets"]["report_curl"] =~ "Authorization: Bearer $CANARY_READ_KEY"
      assert body["snippets"]["report_curl"] =~ "/api/v1/report?window=1h"
      assert body["snippets"]["service_query_curl"] =~ "Authorization: Bearer $CANARY_READ_KEY"
      assert body["snippets"]["service_query_curl"] =~ "service=billing-api&window=1h"
      assert body["snippets"]["elixir_logger"] =~ "service: \"billing-api\""
      assert body["snippets"]["typescript_init"] =~ "service: \"billing-api\""

      error_conn =
        build_conn()
        |> authenticate(body["api_key"]["key"])
        |> post("/api/v1/errors", %{
          "service" => "billing-api",
          "environment" => "staging",
          "error_class" => "RuntimeError",
          "message" => "canary onboarding check"
        })

      assert json_response(error_conn, 201)["id"] =~ ~r/^ERR-/

      report_conn =
        build_conn()
        |> authenticate(admin_raw_key)
        |> get("/api/v1/report?window=1h")

      report = json_response(report_conn, 200)

      assert Enum.any?(report["targets"], &(&1["service"] == "billing-api"))
      assert Enum.any?(report["error_groups"], &(&1["service"] == "billing-api"))
    end

    test "returns validation errors without creating partial artifacts", %{conn: conn} do
      before_targets = Repo.aggregate(Target, :count, :id)
      before_keys = Repo.aggregate(ApiKey, :count, :id)

      conn =
        post(conn, "/api/v1/service-onboarding", %{
          "service" => "bad-service",
          "url" => "ftp://example.com/health"
        })

      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["errors"]["url"] == ["scheme must be http or https"]
      assert Repo.aggregate(Target, :count, :id) == before_targets
      assert Repo.aggregate(ApiKey, :count, :id) == before_keys
    end

    test "rejects duplicate onboarding for the same service without creating a new key", %{
      conn: conn,
      admin_raw_key: admin_raw_key
    } do
      first =
        post(conn, "/api/v1/service-onboarding", %{
          "service" => "linejam",
          "url" => "https://example.com/linejam/health"
        })

      assert json_response(first, 201)["service"] == "linejam"

      duplicate_conn =
        build_conn()
        |> authenticate(admin_raw_key)
        |> post("/api/v1/service-onboarding", %{
          "service" => "linejam",
          "url" => "https://example.com/linejam/other-health"
        })

      body = json_response(duplicate_conn, 422)

      assert body["code"] == "validation_error"
      assert body["errors"]["service"] == ["already has a health target"]
      assert Repo.aggregate(Target, :count, :id) == 1
      assert Repo.aggregate(ApiKey, :count, :id) == 2
    end
  end
end
