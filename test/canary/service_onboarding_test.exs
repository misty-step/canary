defmodule Canary.ServiceOnboardingTest do
  use Canary.DataCase

  import Canary.Fixtures

  alias Canary.Repo
  alias Canary.Schemas.{ApiKey, Target}
  alias Canary.ServiceOnboarding
  alias Canary.ServiceOnboarding.Connect
  alias Canary.ServiceOnboarding.Payload
  alias Canary.ServiceOnboarding.Request

  setup do
    clean_status_tables()
    Repo.delete_all(ApiKey)
    :ok
  end

  describe "connect/3" do
    test "creates a target, key, and exact snippets" do
      assert {:ok, result} =
               ServiceOnboarding.connect(
                 %{
                   "service" => "billing-api",
                   "url" => "https://example.com/billing/health",
                   "environment" => "staging",
                   "interval_ms" => 30_000
                 },
                 "https://canary.example.com"
               )

      assert result.service == "billing-api"
      assert result.api_key.name == "billing-api-ingest"
      assert result.target.name == "billing-api"
      assert result.target.interval_ms == 30_000
      assert result.links.dashboard == "https://canary.example.com/dashboard"
      assert result.links.report == "https://canary.example.com/api/v1/report?window=1h"

      assert result.snippets.error_ingest_curl =~ "https://canary.example.com/api/v1/errors"
      assert result.snippets.error_ingest_curl =~ "Authorization: Bearer #{result.api_key.key}"
      assert result.snippets.error_ingest_curl =~ "\"service\":\"billing-api\""
      assert result.snippets.error_ingest_curl =~ "\"environment\":\"staging\""

      assert result.snippets.service_query_curl =~ "service=billing-api&window=1h"
      assert result.snippets.elixir_logger =~ "service: \"billing-api\""
      assert result.snippets.elixir_logger =~ "environment: \"staging\""
      assert result.snippets.typescript_init =~ "apiKey: \"#{result.api_key.key}\""
      assert result.snippets.typescript_init =~ "service: \"billing-api\""
    end

    test "rolls back the target when key generation fails" do
      failing_key_generator = fn _name, _env ->
        {:error, Ecto.Changeset.change(%ApiKey{}, %{name: nil})}
      end

      assert {:error, :internal} =
               ServiceOnboarding.connect(
                 %{
                   "service" => "volume-api",
                   "url" => "https://example.com/volume/health"
                 },
                 "https://canary.example.com",
                 generate_key: failing_key_generator
               )

      assert Repo.aggregate(Target, :count, :id) == 0
      assert Repo.aggregate(ApiKey, :count, :id) == 0
    end

    test "rejects onboarding when a target already exists for the service" do
      assert {:ok, _result} =
               ServiceOnboarding.connect(
                 %{
                   "service" => "linejam",
                   "url" => "https://example.com/linejam/health"
                 },
                 "https://canary.example.com"
               )

      assert {:error, {:validation, changeset}} =
               ServiceOnboarding.connect(
                 %{
                   "service" => "linejam",
                   "url" => "https://example.com/linejam/other-health"
                 },
                 "https://canary.example.com"
               )

      assert errors_on(changeset) == %{service: ["already has a health target"]}
      assert Repo.aggregate(Target, :count, :id) == 1
      assert Repo.aggregate(ApiKey, :count, :id) == 1
    end

    test "rechecks duplicate targets at write time" do
      assert {:ok, request} =
               Request.apply(%{
                 "service" => "billing-api",
                 "url" => "https://example.com/billing/health"
               })

      assert {:ok, _result} =
               ServiceOnboarding.connect(
                 %{
                   "service" => "billing-api",
                   "url" => "https://example.com/billing/other-health"
                 },
                 "https://canary.example.com"
               )

      assert {:error, changeset} = Connect.connect(request)
      assert errors_on(changeset) == %{service: ["already has a health target"]}
      assert Repo.aggregate(Target, :count, :id) == 1
      assert Repo.aggregate(ApiKey, :count, :id) == 1
    end
  end

  describe "Request.apply/1" do
    test "rejects a duplicate monitored URL" do
      assert {:ok, _result} =
               ServiceOnboarding.connect(
                 %{
                   "service" => "billing-api",
                   "url" => "https://example.com/billing/health"
                 },
                 "https://canary.example.com"
               )

      assert {:error, changeset} =
               Request.apply(%{
                 "service" => "reporting-api",
                 "url" => "https://example.com/billing/health"
               })

      assert errors_on(changeset) == %{url: ["is already monitored"]}
    end
  end

  describe "Payload.render/2" do
    test "renders the onboarding contract from a connection result" do
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      request = %Request{
        service: "billing-api",
        url: "https://example.com/billing/health",
        environment: "staging",
        interval_ms: 30_000
      }

      target = %Target{
        id: "TGT-billing-api",
        name: "billing-api",
        service: "billing-api",
        url: "https://example.com/billing/health",
        method: "GET",
        interval_ms: 30_000,
        timeout_ms: 10_000,
        expected_status: "200",
        active: 1,
        created_at: now
      }

      api_key = %ApiKey{
        id: "KEY-billing-api",
        name: "billing-api-ingest",
        key_prefix: "cnry_live",
        created_at: now
      }

      result = %Connect.Result{
        request: request,
        target: target,
        api_key: api_key,
        raw_key: "raw-key-123"
      }

      payload = Payload.render(result, "https://canary.example.com")

      assert payload.service == "billing-api"
      assert payload.api_key.key == "raw-key-123"
      assert payload.target.id == "TGT-billing-api"
      assert payload.links.dashboard == "https://canary.example.com/dashboard"
      assert payload.links.service_query =~ "service=billing-api&window=1h"
      assert payload.snippets.error_ingest_curl =~ "Authorization: Bearer raw-key-123"
      assert payload.snippets.typescript_init =~ "environment: \"staging\""
    end
  end
end
