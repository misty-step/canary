defmodule CanaryWeb.TargetControllerTest do
  use CanaryWeb.ConnCase

  setup %{conn: conn} do
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "CRUD /api/v1/targets" do
    test "create, list, delete cycle", %{conn: conn} do
      # Create
      create_conn =
        post(conn, "/api/v1/targets", %{
          "url" => "https://example.com",
          "name" => "Example",
          "allow_private" => true
        })

      created = json_response(create_conn, 201)
      assert created["id"] =~ ~r/^TGT-/
      assert created["name"] == "Example"
      assert created["url"] == "https://example.com"
      assert created["active"] == true

      # List — sandbox preserves seeded targets, so check membership
      list_conn = get(conn, "/api/v1/targets")
      body = json_response(list_conn, 200)
      assert Enum.any?(body["targets"], &(&1["id"] == created["id"]))

      # Delete
      delete_conn = delete(conn, "/api/v1/targets/#{created["id"]}")
      assert response(delete_conn, 204)

      # Verify deleted
      list_conn2 = get(conn, "/api/v1/targets")
      refute Enum.any?(json_response(list_conn2, 200)["targets"], &(&1["id"] == created["id"]))
    end

    test "create returns 422 for invalid URL", %{conn: conn} do
      conn = post(conn, "/api/v1/targets", %{"url" => "ftp://bad", "name" => "Bad"})

      body = json_response(conn, 422)
      assert body["code"] == "validation_error"
    end

    test "delete returns 404 for missing target", %{conn: conn} do
      conn = delete(conn, "/api/v1/targets/TGT-nonexistent")
      assert json_response(conn, 404)["code"] == "not_found"
    end
  end
end
