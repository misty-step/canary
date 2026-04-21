defmodule CanaryWeb.OpenAPIControllerTest do
  use CanaryWeb.ConnCase

  @rate_limited_operations [
    {"/metrics", "get"},
    {"/api/v1/errors", "post"},
    {"/api/v1/check-ins", "post"},
    {"/api/v1/query", "get"},
    {"/api/v1/errors/{id}", "get"},
    {"/api/v1/report", "get"},
    {"/api/v1/timeline", "get"},
    {"/api/v1/webhook-deliveries", "get"},
    {"/api/v1/status", "get"},
    {"/api/v1/health-status", "get"},
    {"/api/v1/targets/{id}/checks", "get"},
    {"/api/v1/incidents", "get"},
    {"/api/v1/incidents/{id}", "get"},
    {"/api/v1/incidents/{incident_id}/annotations", "get"},
    {"/api/v1/incidents/{incident_id}/annotations", "post"},
    {"/api/v1/groups/{group_hash}/annotations", "get"},
    {"/api/v1/groups/{group_hash}/annotations", "post"}
  ]

  test "serves the public OpenAPI contract and matches router operations", %{conn: conn} do
    body = openapi_body(conn)

    assert body["openapi"] == "3.1.0"
    assert body["info"]["title"] == "Canary API"
    assert body["info"]["version"] == "0.1.0"
    assert get_in(body, ["components", "securitySchemes", "bearerAuth", "scheme"]) == "bearer"
    assert spec_operations(body) == router_contract_operations()
  end

  test "documents the canonical agent replay pattern", %{conn: conn} do
    body = openapi_body(conn)

    guide = get_in(body, ["info", "x-agent-guide"])

    assert is_map(guide)
    assert is_binary(guide["summary"])
    assert is_binary(guide["crash_recovery"])
    assert is_binary(guide["cursor_precedence"])

    assert guide["summary"] =~ "webhook"
    assert guide["summary"] =~ "timeline"
    assert guide["crash_recovery"] =~ "persisted cursor"
    assert guide["cursor_precedence"] =~ "`after`"
    assert guide["cursor_precedence"] =~ "`cursor`"
  end

  test "avoids unconstrained additionalProperties escape hatches", %{conn: conn} do
    conn |> openapi_body() |> assert_no_true_additional_properties()
  end

  test "documents rate limiting on every rate-limited operation", %{conn: conn} do
    body = openapi_body(conn)

    Enum.each(@rate_limited_operations, fn {path, method} ->
      assert_rate_limited(body, path, method)
    end)
  end

  test "documents API key scopes and forbidden responses", %{conn: conn} do
    body = openapi_body(conn)

    assert get_in(body, ["components", "schemas", "ApiKeyScope", "enum"]) ==
             ["admin", "ingest-only", "read-only"]

    assert get_in(body, ["components", "schemas", "ApiKey", "properties", "scope", "$ref"]) ==
             "#/components/schemas/ApiKeyScope"

    assert get_in(body, [
             "components",
             "schemas",
             "ApiKeyCreateRequest",
             "properties",
             "scope",
             "$ref"
           ]) ==
             "#/components/schemas/ApiKeyScope"

    assert get_in(body, ["paths", "/api/v1/errors", "post", "responses", "403", "$ref"]) ==
             "#/components/responses/ForbiddenProblem"

    assert get_in(body, ["paths", "/api/v1/keys", "post", "responses", "403", "$ref"]) ==
             "#/components/responses/ForbiddenProblem"
  end

  defp openapi_body(conn) do
    conn
    |> get("/api/v1/openapi.json")
    |> json_response(200)
  end

  defp spec_operations(body) do
    body["paths"]
    |> Enum.flat_map(fn {path, operations} ->
      operations
      |> Enum.filter(fn {method, _definition} -> method in ~w(get post delete) end)
      |> Enum.map(fn {method, _definition} -> {path, method} end)
    end)
    |> MapSet.new()
  end

  defp router_contract_operations do
    CanaryWeb.Router
    |> Phoenix.Router.routes()
    |> Enum.filter(fn %{verb: verb, path: path} ->
      verb in [:get, :post, :delete] and contract_path?(path)
    end)
    |> Enum.map(fn %{verb: verb, path: path} ->
      {normalize_path(path), Atom.to_string(verb)}
    end)
    |> MapSet.new()
  end

  defp contract_path?(path) do
    path in ["/healthz", "/readyz", "/metrics"] or String.starts_with?(path, "/api/v1/")
  end

  defp normalize_path(path) do
    Regex.replace(~r/:([a-zA-Z_]+)/, path, "{\\1}")
  end

  defp assert_no_true_additional_properties(value) when is_list(value) do
    Enum.each(value, &assert_no_true_additional_properties/1)
  end

  defp assert_no_true_additional_properties(%{"additionalProperties" => true} = value) do
    flunk("unexpected additionalProperties: true in #{inspect(value)}")
  end

  defp assert_no_true_additional_properties(value) when is_map(value) do
    Enum.each(value, fn {_key, nested} ->
      assert_no_true_additional_properties(nested)
    end)
  end

  defp assert_no_true_additional_properties(_value), do: :ok

  defp assert_rate_limited(body, path, method) do
    assert get_in(body, ["paths", path, method, "responses", "429", "$ref"]) ==
             "#/components/responses/RateLimitedProblem"
  end
end
