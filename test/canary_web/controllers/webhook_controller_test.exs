defmodule CanaryWeb.WebhookControllerTest do
  use CanaryWeb.ConnCase

  setup %{conn: conn} do
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "CRUD /api/v1/webhooks" do
    test "create returns secret, list omits it, delete works", %{conn: conn} do
      # Create
      create_conn =
        post(conn, "/api/v1/webhooks", %{
          "url" => "https://example.com/hook",
          "events" => ["error.new_class", "health_check.down"]
        })

      created = json_response(create_conn, 201)
      assert created["id"] =~ ~r/^WHK-/
      assert is_binary(created["secret"])
      assert String.length(created["secret"]) == 32
      assert created["events"] == ["error.new_class", "health_check.down"]

      # List (no secret exposed)
      list_conn = get(conn, "/api/v1/webhooks")
      body = json_response(list_conn, 200)
      wh = Enum.find(body["webhooks"], &(&1["id"] == created["id"]))
      assert wh
      refute Map.has_key?(wh, "secret")

      # Delete
      delete_conn = delete(conn, "/api/v1/webhooks/#{created["id"]}")
      assert response(delete_conn, 204)

      # Verify deleted
      list_conn2 = get(conn, "/api/v1/webhooks")
      assert json_response(list_conn2, 200)["webhooks"] == []
    end

    test "create rejects invalid event types", %{conn: conn} do
      conn =
        post(conn, "/api/v1/webhooks", %{
          "url" => "https://example.com/hook",
          "events" => ["bogus.event"]
        })

      body = json_response(conn, 422)
      assert body["code"] == "validation_error"
      assert body["detail"] =~ "bogus.event"
    end

    test "delete returns 404 for missing webhook", %{conn: conn} do
      conn = delete(conn, "/api/v1/webhooks/WHK-nonexistent")
      assert json_response(conn, 404)["code"] == "not_found"
    end
  end
end
