defmodule CanaryWeb.KeyControllerTest do
  use CanaryWeb.ConnCase

  setup %{conn: conn} do
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "POST /api/v1/keys" do
    test "returns raw key on creation", %{conn: conn} do
      conn = post(conn, "/api/v1/keys", %{"name" => "new-key"})

      body = json_response(conn, 201)
      assert body["id"] =~ ~r/^KEY-/
      assert body["name"] == "new-key"
      assert body["key"] =~ ~r/^sk_live_/
      assert body["key_prefix"]
      assert body["warning"] =~ "Store this key"
    end
  end

  describe "GET /api/v1/keys" do
    test "returns prefixes, not raw keys", %{conn: conn} do
      # Create a key via API so it exists
      post(conn, "/api/v1/keys", %{"name" => "listed-key"})

      conn = get(conn, "/api/v1/keys")
      body = json_response(conn, 200)

      assert is_list(body["keys"])
      assert length(body["keys"]) >= 1

      Enum.each(body["keys"], fn k ->
        assert k["key_prefix"]
        refute Map.has_key?(k, "key")
        refute Map.has_key?(k, "key_hash")
      end)
    end
  end

  describe "POST /api/v1/keys/:id/revoke" do
    test "revokes an existing key", %{conn: conn} do
      create_body = json_response(post(conn, "/api/v1/keys", %{"name" => "doomed"}), 201)

      conn = post(conn, "/api/v1/keys/#{create_body["id"]}/revoke")
      assert json_response(conn, 200)["status"] == "revoked"
    end

    test "returns 404 for missing key", %{conn: conn} do
      conn = post(conn, "/api/v1/keys/KEY-nonexistent/revoke")
      assert json_response(conn, 404)["code"] == "not_found"
    end
  end
end
