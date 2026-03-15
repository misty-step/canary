defmodule CanaryWeb.QueryControllerTest do
  use CanaryWeb.ConnCase

  setup %{conn: conn} do
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "GET /api/v1/query" do
    test "returns errors by service", %{conn: conn} do
      # Ingest an error first
      post(conn, "/api/v1/errors", %{
        "service" => "query-test",
        "error_class" => "TestError",
        "message" => "test message"
      })

      conn = get(conn, "/api/v1/query?service=query-test")
      body = json_response(conn, 200)
      assert body["service"] == "query-test"
      assert is_list(body["groups"])
    end

    test "returns errors by class", %{conn: conn} do
      conn = get(conn, "/api/v1/query?group_by=error_class")
      body = json_response(conn, 200)
      assert is_list(body["groups"])
    end

    test "422 when no service or group_by", %{conn: conn} do
      conn = get(conn, "/api/v1/query")
      assert json_response(conn, 422)["code"] == "validation_error"
    end
  end

  describe "GET /api/v1/errors/:id" do
    test "returns error detail", %{conn: conn} do
      # Ingest an error
      create_body =
        conn
        |> post("/api/v1/errors", %{
          "service" => "detail-test",
          "error_class" => "DetailError",
          "message" => "detail message"
        })
        |> json_response(201)

      conn = get(conn, "/api/v1/errors/#{create_body["id"]}")
      body = json_response(conn, 200)
      assert body["id"] == create_body["id"]
      assert body["error_class"] == "DetailError"
      assert body["service"] == "detail-test"
    end

    test "404 for missing error", %{conn: conn} do
      conn = get(conn, "/api/v1/errors/ERR-nonexistent")
      assert json_response(conn, 404)["code"] == "not_found"
    end
  end
end
