defmodule CanaryWeb.ServiceOnboardingContractTest do
  use CanaryWeb.ConnCase

  import Canary.Fixtures

  alias Canary.Repo
  alias Canary.Schemas.{ApiKey, Target}

  @moduletag :contract
  @route "/api/v1/service-onboarding"
  @base_url "http://www.example.com"

  setup do
    clean_status_tables()
    Repo.delete_all(ApiKey)
    :ok
  end

  describe "POST /api/v1/service-onboarding" do
    test "returns 401 problem details without bearer auth", %{conn: conn} do
      before_targets = Repo.aggregate(Target, :count, :id)
      before_keys = Repo.aggregate(ApiKey, :count, :id)

      conn =
        post(conn, @route, %{
          "service" => "billing-api",
          "url" => "https://example.com/billing/health"
        })

      body = json_response(conn, 401)

      assert body["code"] == "invalid_api_key"
      assert body["status"] == 401
      assert body["title"] == "Invalid API Key"
      assert body["detail"] == "Missing Authorization header. Use: Bearer sk_..."
      assert body["type"] == "https://canary.dev/problems/invalid-api-key"
      assert is_binary(body["request_id"])
      assert get_resp_header(conn, "content-type") == ["application/problem+json; charset=utf-8"]
      assert Repo.aggregate(Target, :count, :id) == before_targets
      assert Repo.aggregate(ApiKey, :count, :id) == before_keys
    end

    test "returns the documented onboarding payload contract", %{conn: conn} do
      {admin_raw_key, _key} = create_api_key("admin")

      conn =
        conn
        |> authenticate(admin_raw_key)
        |> post(@route, %{
          "service" => "billing api",
          "url" => "https://example.com/billing/health",
          "environment" => "staging",
          "interval_ms" => 30_000
        })

      body = json_response(conn, 201)
      raw_key = body["api_key"]["key"]

      assert Enum.sort(Map.keys(body)) == ~w(api_key links service snippets target)
      assert body["service"] == "billing api"

      assert Enum.sort(Map.keys(body["api_key"])) ==
               ~w(created_at id key key_prefix name scope warning)

      assert body["api_key"]["id"] =~ ~r/^KEY-/
      assert body["api_key"]["name"] == "billing api-ingest"
      assert body["api_key"]["scope"] == "ingest-only"
      assert body["api_key"]["key"] =~ ~r/^sk_live_/
      assert body["api_key"]["key_prefix"] =~ ~r/^sk_live_/
      assert body["api_key"]["warning"] == "Store this key securely. It will not be shown again."
      assert is_binary(body["api_key"]["created_at"])

      assert Enum.sort(Map.keys(body["target"])) ==
               ~w(active created_at expected_status id interval_ms method name service timeout_ms url)

      assert body["target"]["id"] =~ ~r/^TGT-/
      assert body["target"]["name"] == "billing api"
      assert body["target"]["service"] == "billing api"
      assert body["target"]["url"] == "https://example.com/billing/health"
      assert body["target"]["method"] == "GET"
      assert body["target"]["interval_ms"] == 30_000
      assert body["target"]["timeout_ms"] == 10000
      assert body["target"]["expected_status"] == "200"
      assert body["target"]["active"] == true
      assert is_binary(body["target"]["created_at"])

      assert body["links"] == %{
               "dashboard" => "#{@base_url}/dashboard",
               "report" => "#{@base_url}/api/v1/report?window=1h",
               "service_query" => "#{@base_url}/api/v1/query?service=billing+api&window=1h"
             }

      assert Enum.sort(Map.keys(body["snippets"])) ==
               ~w(elixir_logger error_ingest_curl report_curl service_query_curl typescript_init)

      assert body["snippets"]["report_curl"] ==
               """
               curl "#{body["links"]["report"]}" \\
                 -H "Authorization: Bearer $CANARY_READ_KEY"
               """
               |> String.trim()

      assert body["snippets"]["service_query_curl"] ==
               """
               curl "#{body["links"]["service_query"]}" \\
                 -H "Authorization: Bearer $CANARY_READ_KEY"
               """
               |> String.trim()

      assert body["snippets"]["elixir_logger"] ==
               """
               CanarySdk.attach(
                 endpoint: "#{@base_url}",
                 api_key: "#{raw_key}",
                 service: "billing api",
                 environment: "staging"
               )
               """
               |> String.trim()

      assert body["snippets"]["typescript_init"] ==
               """
               import { initCanary } from "@canary-obs/sdk";

               initCanary({
                 endpoint: "#{@base_url}",
                 apiKey: "#{raw_key}",
                 service: "billing api",
                 environment: "staging"
               });
               """
               |> String.trim()

      error_ingest_snippet = body["snippets"]["error_ingest_curl"]

      assert error_ingest_snippet =~ "curl -X POST #{@base_url}/api/v1/errors"
      assert error_ingest_snippet =~ ~s(-H "Authorization: Bearer #{raw_key}")
      assert error_ingest_snippet =~ ~s(-H "Content-Type: application/json")

      assert extract_ingest_payload(error_ingest_snippet) == %{
               "service" => "billing api",
               "environment" => "staging",
               "error_class" => "RuntimeError",
               "message" => "canary onboarding check",
               "severity" => "error",
               "context" => %{"source" => "service-onboarding"}
             }
    end

    test "returns 422 problem details for duplicate services", %{conn: conn} do
      {admin_raw_key, _key} = create_api_key("admin")

      create_target_with_state("billing-api", "unknown")

      conn =
        conn
        |> authenticate(admin_raw_key)
        |> post(@route, %{
          "service" => "billing-api",
          "url" => "https://example.com/billing/other-health"
        })

      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["status"] == 422
      assert body["title"] == "Validation Error"
      assert body["detail"] == "Invalid service onboarding request."
      assert body["errors"] == %{"service" => ["already has a health target"]}
      assert is_binary(body["request_id"])
    end
  end

  describe "GET /api/v1/openapi.json onboarding contract" do
    test "documents the onboarding route, schemas, and auth failure responses", %{conn: conn} do
      body =
        conn
        |> get("/api/v1/openapi.json")
        |> json_response(200)

      operation = get_in(body, ["paths", "/api/v1/service-onboarding", "post"])

      assert operation["summary"] == "Connect a service and return onboarding snippets"

      assert get_in(operation, ["requestBody", "required"]) == true

      assert get_in(operation, ["requestBody", "content", "application/json", "schema", "$ref"]) ==
               "#/components/schemas/ServiceOnboardingRequest"

      assert get_in(operation, [
               "responses",
               "201",
               "content",
               "application/json",
               "schema",
               "$ref"
             ]) == "#/components/schemas/ServiceOnboardingResponse"

      assert get_in(operation, ["responses", "401", "$ref"]) ==
               "#/components/responses/UnauthorizedProblem"

      assert get_in(operation, ["responses", "403", "$ref"]) ==
               "#/components/responses/ForbiddenProblem"

      assert get_in(operation, ["responses", "422", "$ref"]) ==
               "#/components/responses/ValidationProblem"

      assert get_in(operation, ["responses", "500", "$ref"]) ==
               "#/components/responses/InternalProblem"

      assert get_in(body, ["components", "securitySchemes", "bearerAuth", "scheme"]) == "bearer"
    end
  end

  defp extract_ingest_payload(snippet) do
    [json] = Regex.run(~r/<<'JSON'\n(?<json>.*)\nJSON/s, snippet, capture: :all_but_first)
    Jason.decode!(json)
  end
end
