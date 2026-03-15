defmodule CanaryWeb.ErrorControllerTest do
  use CanaryWeb.ConnCase

  @valid_payload %{
    "service" => "test-svc",
    "error_class" => "RuntimeError",
    "message" => "something went wrong"
  }

  describe "POST /api/v1/errors" do
    test "401 with RFC 9457 body when no auth header", %{conn: conn} do
      conn = post(conn, "/api/v1/errors", @valid_payload)

      assert json_response(conn, 401)["code"] == "invalid_api_key"
      assert json_response(conn, 401)["status"] == 401
      assert json_response(conn, 401)["type"] =~ "invalid-api-key"
    end

    test "201 with error ID on valid payload", %{conn: conn} do
      {raw_key, _key} = create_api_key()

      conn =
        conn
        |> authenticate(raw_key)
        |> post("/api/v1/errors", @valid_payload)

      body = json_response(conn, 201)
      assert body["id"] =~ ~r/^ERR-/
      assert body["group_hash"]
      assert is_boolean(body["is_new_class"])
    end

    test "422 with validation errors when required fields missing", %{conn: conn} do
      {raw_key, _key} = create_api_key()

      conn =
        conn
        |> authenticate(raw_key)
        |> post("/api/v1/errors", %{})

      body = json_response(conn, 422)
      assert body["code"] == "validation_error"
      assert body["status"] == 422
      assert body["errors"]
    end
  end
end
