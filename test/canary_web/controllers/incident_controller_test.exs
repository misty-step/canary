defmodule CanaryWeb.IncidentControllerTest do
  use CanaryWeb.ConnCase

  import Canary.Fixtures

  setup %{conn: conn} do
    clean_status_tables()
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "GET /api/v1/incidents" do
    test "returns active incidents", %{conn: conn} do
      create_target_with_state("svc-inc", "down")
      incident = create_incident("svc-inc")

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "health_transition",
        signal_ref: "TGT-svc-inc",
        attached_at: DateTime.utc_now() |> DateTime.to_iso8601()
      })

      conn = get(conn, "/api/v1/incidents")
      body = json_response(conn, 200)
      assert is_list(body["incidents"])
      ids = Enum.map(body["incidents"], & &1["id"])
      assert incident.id in ids
    end

    test "without_annotation excludes incident with matching annotation", %{conn: conn} do
      create_target_with_state("svc-excl", "down")
      incident = create_incident("svc-excl")

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "health_transition",
        signal_ref: "TGT-svc-excl",
        attached_at: DateTime.utc_now() |> DateTime.to_iso8601()
      })

      create_annotation(:incident, incident.id, action: "acknowledged")

      conn = get(conn, "/api/v1/incidents?without_annotation=acknowledged")
      body = json_response(conn, 200)
      ids = Enum.map(body["incidents"], & &1["id"])
      refute incident.id in ids
    end

    test "with_annotation includes incident with matching annotation", %{conn: conn} do
      create_target_with_state("svc-incl", "down")
      incident = create_incident("svc-incl")

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "health_transition",
        signal_ref: "TGT-svc-incl",
        attached_at: DateTime.utc_now() |> DateTime.to_iso8601()
      })

      create_annotation(:incident, incident.id, action: "acknowledged")

      conn = get(conn, "/api/v1/incidents?with_annotation=acknowledged")
      body = json_response(conn, 200)
      ids = Enum.map(body["incidents"], & &1["id"])
      assert incident.id in ids
    end

    test "without_annotation includes incident lacking the annotation", %{conn: conn} do
      create_target_with_state("svc-noann", "down")
      incident = create_incident("svc-noann")

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "health_transition",
        signal_ref: "TGT-svc-noann",
        attached_at: DateTime.utc_now() |> DateTime.to_iso8601()
      })

      conn = get(conn, "/api/v1/incidents?without_annotation=acknowledged")
      body = json_response(conn, 200)
      ids = Enum.map(body["incidents"], & &1["id"])
      assert incident.id in ids
    end

    test "returns 401 without auth", %{conn: conn} do
      conn =
        conn
        |> delete_req_header("authorization")
        |> get("/api/v1/incidents")

      assert json_response(conn, 401)["code"] == "invalid_api_key"
    end
  end
end
