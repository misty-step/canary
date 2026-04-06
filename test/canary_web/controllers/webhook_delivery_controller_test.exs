defmodule CanaryWeb.WebhookDeliveryControllerTest do
  use CanaryWeb.ConnCase

  alias Canary.WebhookDeliveries

  setup %{conn: conn} do
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "GET /api/v1/webhook-deliveries" do
    test "lists deliveries newest first with filters and bounded fields", %{conn: conn} do
      assert :ok =
               WebhookDeliveries.create_pending(
                 "DLV-old",
                 "WHK-alpha",
                 "error.new_class",
                 "2026-04-02T10:00:00Z"
               )

      assert :ok = WebhookDeliveries.mark_attempt("DLV-old", "2026-04-02T10:00:01Z")
      assert :ok = WebhookDeliveries.mark_delivered("DLV-old", "2026-04-02T10:00:02Z")

      assert :ok =
               WebhookDeliveries.create_suppressed(
                 "DLV-suppressed",
                 "WHK-alpha",
                 "error.new_class",
                 "cooldown",
                 "2026-04-02T10:05:00Z"
               )

      assert :ok =
               WebhookDeliveries.create_pending(
                 "DLV-other",
                 "WHK-beta",
                 "incident.updated",
                 "2026-04-02T10:10:00Z"
               )

      assert :ok = WebhookDeliveries.mark_attempt("DLV-other", "2026-04-02T10:10:01Z")

      assert :ok =
               WebhookDeliveries.mark_discarded("DLV-other", "http_500", "2026-04-02T10:10:02Z")

      conn =
        get(conn, "/api/v1/webhook-deliveries", %{
          "webhook_id" => "WHK-alpha",
          "limit" => "2"
        })

      body = json_response(conn, 200)

      assert body["returned_count"] == 2
      assert Enum.map(body["deliveries"], & &1["delivery_id"]) == ["DLV-suppressed", "DLV-old"]
      assert is_nil(body["cursor"])

      [suppressed, delivered] = body["deliveries"]

      assert suppressed["status"] == "suppressed"
      assert suppressed["reason"] == "cooldown"
      assert suppressed["completed_at"] == "2026-04-02T10:05:00Z"
      assert suppressed["attempt_count"] == 0

      assert delivered["status"] == "delivered"
      assert delivered["attempt_count"] == 1
      assert delivered["first_attempt_at"] == "2026-04-02T10:00:01Z"
      assert delivered["last_attempt_at"] == "2026-04-02T10:00:01Z"
      assert delivered["delivered_at"] == "2026-04-02T10:00:02Z"
      assert delivered["completed_at"] == "2026-04-02T10:00:02Z"
    end

    test "filters by status", %{conn: conn} do
      assert :ok =
               WebhookDeliveries.create_suppressed(
                 "DLV-cooldown",
                 "WHK-alpha",
                 "error.new_class",
                 "cooldown",
                 "2026-04-02T11:00:00Z"
               )

      assert :ok =
               WebhookDeliveries.create_pending(
                 "DLV-discarded",
                 "WHK-alpha",
                 "error.new_class",
                 "2026-04-02T11:01:00Z"
               )

      assert :ok = WebhookDeliveries.mark_attempt("DLV-discarded", "2026-04-02T11:01:01Z")

      assert :ok =
               WebhookDeliveries.mark_discarded(
                 "DLV-discarded",
                 "http_500",
                 "2026-04-02T11:01:02Z"
               )

      conn = get(conn, "/api/v1/webhook-deliveries", %{"status" => "suppressed"})
      body = json_response(conn, 200)

      assert body["returned_count"] == 1
      assert Enum.map(body["deliveries"], & &1["delivery_id"]) == ["DLV-cooldown"]
    end

    test "supports cursor pagination", %{conn: conn} do
      assert :ok =
               WebhookDeliveries.create_suppressed(
                 "DLV-3",
                 "WHK-alpha",
                 "error.new_class",
                 "cooldown",
                 "2026-04-02T12:03:00Z"
               )

      assert :ok =
               WebhookDeliveries.create_suppressed(
                 "DLV-2",
                 "WHK-alpha",
                 "error.new_class",
                 "cooldown",
                 "2026-04-02T12:02:00Z"
               )

      assert :ok =
               WebhookDeliveries.create_suppressed(
                 "DLV-1",
                 "WHK-alpha",
                 "error.new_class",
                 "cooldown",
                 "2026-04-02T12:01:00Z"
               )

      first_conn = get(conn, "/api/v1/webhook-deliveries", %{"limit" => "2"})
      first_body = json_response(first_conn, 200)

      assert Enum.map(first_body["deliveries"], & &1["delivery_id"]) == ["DLV-3", "DLV-2"]
      assert is_binary(first_body["cursor"])

      second_conn =
        get(conn, "/api/v1/webhook-deliveries", %{
          "limit" => "2",
          "cursor" => first_body["cursor"]
        })

      second_body = json_response(second_conn, 200)

      assert Enum.map(second_body["deliveries"], & &1["delivery_id"]) == ["DLV-1"]
      assert is_nil(second_body["cursor"])
    end

    test "rejects invalid limits", %{conn: conn} do
      conn = get(conn, "/api/v1/webhook-deliveries", %{"limit" => "0"})
      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["detail"] =~ "Invalid limit"
      assert body["errors"]["limit"] == ["must be a positive integer no greater than 200"]
    end

    test "rejects invalid status values and filter types", %{conn: conn} do
      invalid_status_conn = get(conn, "/api/v1/webhook-deliveries", %{"status" => "supressed"})
      invalid_status_body = json_response(invalid_status_conn, 422)

      assert invalid_status_body["code"] == "validation_error"

      assert invalid_status_body["errors"]["status"] == [
               "must be one of: pending, retrying, delivered, discarded, suppressed"
             ]

      invalid_type_conn = get(conn, "/api/v1/webhook-deliveries", %{"status" => ["suppressed"]})
      invalid_type_body = json_response(invalid_type_conn, 422)

      assert invalid_type_body["code"] == "validation_error"
      assert invalid_type_body["errors"]["status"] == ["must be a string"]
    end

    test "rejects invalid cursors", %{conn: conn} do
      conn = get(conn, "/api/v1/webhook-deliveries", %{"cursor" => "bogus"})
      body = json_response(conn, 422)

      assert body["code"] == "validation_error"
      assert body["errors"]["cursor"] == ["must be a valid pagination cursor"]
    end
  end
end
