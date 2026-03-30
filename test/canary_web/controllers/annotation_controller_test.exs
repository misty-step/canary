defmodule CanaryWeb.AnnotationControllerTest do
  use CanaryWeb.ConnCase

  import Canary.Fixtures

  setup %{conn: conn} do
    clean_status_tables()
    {raw_key, _key} = create_api_key()
    conn = authenticate(conn, raw_key)
    {:ok, conn: conn}
  end

  describe "POST /api/v1/incidents/:incident_id/annotations" do
    test "creates annotation and returns 201", %{conn: conn} do
      incident = create_incident("test-svc")

      conn =
        post(conn, "/api/v1/incidents/#{incident.id}/annotations", %{
          "agent" => "triage-bot",
          "action" => "acknowledged",
          "metadata" => %{"reason" => "auto-triage"}
        })

      body = json_response(conn, 201)
      assert String.starts_with?(body["id"], "ANN-")
      assert body["incident_id"] == incident.id
      assert body["agent"] == "triage-bot"
      assert body["action"] == "acknowledged"
      assert body["metadata"] == %{"reason" => "auto-triage"}
      assert body["created_at"] != nil
    end

    test "returns 422 when agent is missing", %{conn: conn} do
      incident = create_incident("test-svc")

      conn =
        post(conn, "/api/v1/incidents/#{incident.id}/annotations", %{
          "action" => "acknowledged"
        })

      body = json_response(conn, 422)
      assert body["code"] == "validation_error"
      assert body["errors"]["agent"] == ["is required"]
    end

    test "returns 422 when action is missing", %{conn: conn} do
      incident = create_incident("test-svc")

      conn =
        post(conn, "/api/v1/incidents/#{incident.id}/annotations", %{
          "agent" => "bot"
        })

      body = json_response(conn, 422)
      assert body["code"] == "validation_error"
      assert body["errors"]["action"] == ["is required"]
    end

    test "returns 404 for nonexistent incident", %{conn: conn} do
      conn =
        post(conn, "/api/v1/incidents/INC-nonexistent/annotations", %{
          "agent" => "bot",
          "action" => "acknowledged"
        })

      body = json_response(conn, 404)
      assert body["code"] == "not_found"
    end
  end

  describe "GET /api/v1/incidents/:incident_id/annotations" do
    test "lists annotations for incident", %{conn: conn} do
      incident = create_incident("test-svc")

      post(conn, "/api/v1/incidents/#{incident.id}/annotations", %{
        "agent" => "bot-a",
        "action" => "acknowledged"
      })

      post(conn, "/api/v1/incidents/#{incident.id}/annotations", %{
        "agent" => "bot-b",
        "action" => "triaged"
      })

      conn = get(conn, "/api/v1/incidents/#{incident.id}/annotations")
      body = json_response(conn, 200)
      assert length(body["annotations"]) == 2
      agents = Enum.map(body["annotations"], & &1["agent"])
      assert "bot-a" in agents
      assert "bot-b" in agents
    end

    test "multi-consumer coexistence: two agents annotating same incident", %{conn: conn} do
      incident = create_incident("test-svc")

      post(conn, "/api/v1/incidents/#{incident.id}/annotations", %{
        "agent" => "responder-alpha",
        "action" => "acknowledged"
      })

      post(conn, "/api/v1/incidents/#{incident.id}/annotations", %{
        "agent" => "responder-beta",
        "action" => "acknowledged"
      })

      conn = get(conn, "/api/v1/incidents/#{incident.id}/annotations")
      body = json_response(conn, 200)

      agents = Enum.map(body["annotations"], & &1["agent"])
      assert "responder-alpha" in agents
      assert "responder-beta" in agents
      assert length(body["annotations"]) == 2
    end
  end

  describe "POST /api/v1/groups/:group_hash/annotations" do
    test "creates annotation on error group and returns 201", %{conn: conn} do
      group = create_error_group("test-svc", "RuntimeError", 5)

      conn =
        post(conn, "/api/v1/groups/#{group.group_hash}/annotations", %{
          "agent" => "fix-bot",
          "action" => "fix_deployed"
        })

      body = json_response(conn, 201)
      assert String.starts_with?(body["id"], "ANN-")
      assert body["group_hash"] == group.group_hash
      assert body["action"] == "fix_deployed"
    end

    test "returns 404 for nonexistent group", %{conn: conn} do
      conn =
        post(conn, "/api/v1/groups/nonexistent-hash/annotations", %{
          "agent" => "bot",
          "action" => "ack"
        })

      body = json_response(conn, 404)
      assert body["code"] == "not_found"
    end
  end

  describe "GET /api/v1/groups/:group_hash/annotations" do
    test "lists annotations for error group", %{conn: conn} do
      group = create_error_group("test-svc", "RuntimeError", 5)

      post(conn, "/api/v1/groups/#{group.group_hash}/annotations", %{
        "agent" => "bot",
        "action" => "acknowledged"
      })

      conn = get(conn, "/api/v1/groups/#{group.group_hash}/annotations")
      body = json_response(conn, 200)
      assert length(body["annotations"]) == 1
      assert hd(body["annotations"])["group_hash"] == group.group_hash
    end
  end

  describe "auth" do
    test "returns 401 without auth", %{conn: conn} do
      conn =
        conn
        |> delete_req_header("authorization")
        |> get("/api/v1/incidents/INC-test/annotations")

      assert json_response(conn, 401)["code"] == "invalid_api_key"
    end
  end
end
