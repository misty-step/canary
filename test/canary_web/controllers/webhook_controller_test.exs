defmodule CanaryWeb.WebhookControllerTest do
  use CanaryWeb.ConnCase

  alias Canary.Schemas.{ServiceEvent, WebhookDelivery}

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

    test "create accepts incident event types", %{conn: conn} do
      conn =
        post(conn, "/api/v1/webhooks", %{
          "url" => "https://example.com/hook",
          "events" => ["incident.opened", "incident.updated", "incident.resolved"]
        })

      created = json_response(conn, 201)
      assert created["events"] == ["incident.opened", "incident.updated", "incident.resolved"]
    end

    test "delete returns 404 for missing webhook", %{conn: conn} do
      conn = delete(conn, "/api/v1/webhooks/WHK-nonexistent")
      assert json_response(conn, 404)["code"] == "not_found"
    end

    test "test delivery uses a non-business canary ping event", %{conn: conn} do
      bypass = Bypass.open()
      test_pid = self()

      create_conn =
        post(conn, "/api/v1/webhooks", %{
          "url" => "http://localhost:#{bypass.port}/hook",
          "events" => ["error.new_class"]
        })

      created = json_response(create_conn, 201)
      before_count = Canary.Repo.aggregate(ServiceEvent, :count)

      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        send(test_pid, {:test_delivery, Jason.decode!(body)})
        Plug.Conn.resp(conn, 200, "ok")
      end)

      conn = post(conn, "/api/v1/webhooks/#{created["id"]}/test")

      assert json_response(conn, 200)["status"] == "delivered"

      assert_receive {:test_delivery, %{"event" => "canary.ping", "test" => true}}
      assert Canary.Repo.aggregate(ServiceEvent, :count) == before_count
    end

    test "test delivery failures return problem details", %{conn: conn} do
      bypass = Bypass.open()

      create_conn =
        post(conn, "/api/v1/webhooks", %{
          "url" => "http://localhost:#{bypass.port}/hook",
          "events" => ["error.new_class"]
        })

      created = json_response(create_conn, 201)

      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        Plug.Conn.resp(conn, 500, "nope")
      end)

      conn = post(conn, "/api/v1/webhooks/#{created["id"]}/test")
      body = json_response(conn, 502)

      assert get_resp_header(conn, "content-type") == ["application/problem+json; charset=utf-8"]
      assert body["code"] == "webhook_delivery_failed"
      assert body["detail"] =~ "HTTP 500"
    end

    test "create accepts diagnostic canary.ping event", %{conn: conn} do
      conn =
        post(conn, "/api/v1/webhooks", %{
          "url" => "https://example.com/hook",
          "events" => ["canary.ping"]
        })

      created = json_response(conn, 201)
      assert created["events"] == ["canary.ping"]
    end

    test "lists delivery ledger entries for a webhook", %{conn: conn} do
      create_conn =
        post(conn, "/api/v1/webhooks", %{
          "url" => "https://example.com/hook",
          "events" => ["error.new_class"]
        })

      created = json_response(create_conn, 201)

      now = DateTime.utc_now() |> DateTime.to_iso8601()

      %WebhookDelivery{id: "DLV-1"}
      |> WebhookDelivery.changeset(%{
        webhook_id: created["id"],
        event: "error.new_class",
        status: "delivered",
        attempt_count: 2,
        first_attempted_at: now,
        last_attempted_at: now,
        completed_at: now,
        created_at: now
      })
      |> Canary.Repo.insert!()

      conn = get(conn, "/api/v1/webhooks/#{created["id"]}/deliveries")
      body = json_response(conn, 200)

      assert [%{"delivery_id" => "DLV-1"} = delivery] = body["deliveries"]
      assert delivery["event"] == "error.new_class"
      assert delivery["status"] == "delivered"
      assert delivery["attempt_count"] == 2
      assert delivery["first_attempted_at"] == now
      assert delivery["last_attempted_at"] == now
      assert delivery["completed_at"] == now
    end

    test "returns suppression metadata and respects limit when listing deliveries", %{conn: conn} do
      create_conn =
        post(conn, "/api/v1/webhooks", %{
          "url" => "https://example.com/hook",
          "events" => ["error.new_class"]
        })

      created = json_response(create_conn, 201)
      older = "2026-04-02T22:00:00Z"
      newer = "2026-04-02T22:10:00Z"

      %WebhookDelivery{id: "DLV-old"}
      |> WebhookDelivery.changeset(%{
        webhook_id: created["id"],
        event: "error.new_class",
        status: "suppressed",
        attempt_count: 0,
        suppression_reason: "cooldown",
        completed_at: older,
        created_at: older
      })
      |> Canary.Repo.insert!()

      %WebhookDelivery{id: "DLV-new"}
      |> WebhookDelivery.changeset(%{
        webhook_id: created["id"],
        event: "error.new_class",
        status: "delivered",
        attempt_count: 1,
        last_status_code: 202,
        completed_at: newer,
        created_at: newer
      })
      |> Canary.Repo.insert!()

      conn = get(conn, "/api/v1/webhooks/#{created["id"]}/deliveries?limit=1")
      body = json_response(conn, 200)

      assert [%{"delivery_id" => "DLV-new"} = delivery] = body["deliveries"]
      assert delivery["last_status_code"] == 202
      assert delivery["created_at"] == newer
    end

    test "returns suppressed and discarded ledger rows with operator-visible fields", %{
      conn: conn
    } do
      create_conn =
        post(conn, "/api/v1/webhooks", %{
          "url" => "https://example.com/hook",
          "events" => ["error.new_class"]
        })

      created = json_response(create_conn, 201)
      suppressed_at = "2026-04-02T22:05:00Z"
      discarded_at = "2026-04-02T22:15:00Z"

      %WebhookDelivery{id: "DLV-suppressed"}
      |> WebhookDelivery.changeset(%{
        webhook_id: created["id"],
        event: "error.new_class",
        status: "suppressed",
        attempt_count: 0,
        suppression_reason: "cooldown",
        completed_at: suppressed_at,
        created_at: suppressed_at
      })
      |> Canary.Repo.insert!()

      %WebhookDelivery{id: "DLV-discarded"}
      |> WebhookDelivery.changeset(%{
        webhook_id: created["id"],
        event: "error.new_class",
        status: "discarded",
        attempt_count: 4,
        last_status_code: 500,
        last_error: "HTTP 500",
        first_attempted_at: discarded_at,
        last_attempted_at: discarded_at,
        completed_at: discarded_at,
        created_at: discarded_at
      })
      |> Canary.Repo.insert!()

      conn = get(conn, "/api/v1/webhooks/#{created["id"]}/deliveries")
      body = json_response(conn, 200)

      suppressed = Enum.find(body["deliveries"], &(&1["delivery_id"] == "DLV-suppressed"))
      discarded = Enum.find(body["deliveries"], &(&1["delivery_id"] == "DLV-discarded"))

      assert suppressed["status"] == "suppressed"
      assert suppressed["suppression_reason"] == "cooldown"
      assert suppressed["completed_at"] == suppressed_at

      assert discarded["status"] == "discarded"
      assert discarded["attempt_count"] == 4
      assert discarded["last_status_code"] == 500
      assert discarded["last_error"] == "HTTP 500"
      assert discarded["first_attempted_at"] == discarded_at
      assert discarded["last_attempted_at"] == discarded_at
      assert discarded["completed_at"] == discarded_at
    end

    test "returns 404 when listing deliveries for an unknown webhook", %{conn: conn} do
      conn = get(conn, "/api/v1/webhooks/WHK-missing/deliveries")
      assert json_response(conn, 404)["code"] == "not_found"
    end
  end
end
