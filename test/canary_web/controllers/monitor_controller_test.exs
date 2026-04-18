defmodule CanaryWeb.MonitorControllerTest do
  use CanaryWeb.ConnCase

  setup %{conn: conn} do
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "CRUD /api/v1/monitors" do
    test "create, list, delete cycle", %{conn: conn} do
      create_conn =
        post(conn, "/api/v1/monitors", %{
          "name" => "desktop-active-timer",
          "mode" => "ttl",
          "expected_every_ms" => 90_000
        })

      created = json_response(create_conn, 201)
      assert created["id"] =~ ~r/^MON-/
      assert created["name"] == "desktop-active-timer"
      assert created["service"] == "desktop-active-timer"
      assert created["mode"] == "ttl"

      list_conn = get(conn, "/api/v1/monitors")
      assert Enum.any?(json_response(list_conn, 200)["monitors"], &(&1["id"] == created["id"]))

      delete_conn = delete(conn, "/api/v1/monitors/#{created["id"]}")
      assert response(delete_conn, 204)

      refute Enum.any?(
               json_response(get(conn, "/api/v1/monitors"), 200)["monitors"],
               &(&1["id"] == created["id"])
             )
    end

    test "returns validation errors for bad monitors", %{conn: conn} do
      conn = post(conn, "/api/v1/monitors", %{"name" => "broken", "mode" => "bad"})

      body = json_response(conn, 422)
      assert body["code"] == "validation_error"
    end
  end
end
