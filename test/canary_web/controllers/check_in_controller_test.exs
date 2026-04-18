defmodule CanaryWeb.CheckInControllerTest do
  use CanaryWeb.ConnCase

  import Ecto.Query

  alias Canary.Repo
  alias Canary.Schemas.{Error, MonitorState, ServiceEvent}
  import Canary.Fixtures

  setup %{conn: conn} do
    clean_status_tables()
    create_monitor_with_state("desktop-active-timer", "unknown")
    {ingest_key, _key} = create_api_key("ingester", "ingest-only")
    conn = authenticate(conn, ingest_key)
    {:ok, conn: conn}
  end

  describe "POST /api/v1/check-ins" do
    test "stores the check-in and updates state", %{conn: conn} do
      conn =
        post(conn, "/api/v1/check-ins", %{
          "monitor" => "desktop-active-timer",
          "status" => "alive"
        })

      body = json_response(conn, 201)
      assert body["monitor_id"] == "MON-desktop-active-timer"
      assert body["state"] == "up"
      assert Repo.get!(MonitorState, "MON-desktop-active-timer").state == "up"
      assert Repo.aggregate(Error, :count) == 0

      assert ["health_check.recovered"] ==
               Repo.all(
                 from(e in ServiceEvent,
                   select: e.event,
                   where: e.entity_ref == "MON-desktop-active-timer"
                 )
               )
    end

    test "returns 404 for unknown monitors", %{conn: conn} do
      conn =
        post(conn, "/api/v1/check-ins", %{
          "monitor" => "missing",
          "status" => "alive"
        })

      assert json_response(conn, 404)["code"] == "not_found"
    end

    test "returns validation errors for invalid payloads", %{conn: conn} do
      conn = post(conn, "/api/v1/check-ins", %{"monitor" => "desktop-active-timer"})
      body = json_response(conn, 422)
      assert body["code"] == "validation_error"
      assert body["errors"]["status"] == ["must be one of: alive, in_progress, ok, error"]
    end
  end
end
