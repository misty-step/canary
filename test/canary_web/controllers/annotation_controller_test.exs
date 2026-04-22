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

    test "returns 404 for nonexistent incident", %{conn: conn} do
      conn = get(conn, "/api/v1/incidents/INC-nonexistent/annotations")
      body = json_response(conn, 404)
      assert body["code"] == "not_found"
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
    test "returns 404 for nonexistent group", %{conn: conn} do
      conn = get(conn, "/api/v1/groups/nonexistent-hash/annotations")
      body = json_response(conn, 404)
      assert body["code"] == "not_found"
    end

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

  describe "POST /api/v1/annotations (unified)" do
    test "creates annotation on target subject and returns 201", %{conn: conn} do
      create_target_with_state("api", "up")

      conn =
        post(conn, "/api/v1/annotations", %{
          "subject_type" => "target",
          "subject_id" => "TGT-api",
          "agent" => "triage-bot",
          "action" => "paged",
          "metadata" => %{"ticket" => "OPS-1"}
        })

      body = json_response(conn, 201)
      assert body["subject_type"] == "target"
      assert body["subject_id"] == "TGT-api"
      assert body["agent"] == "triage-bot"
      assert body["metadata"] == %{"ticket" => "OPS-1"}
    end

    test "creates annotation on monitor subject", %{conn: conn} do
      create_monitor_with_state("cron", "alive")

      conn =
        post(conn, "/api/v1/annotations", %{
          "subject_type" => "monitor",
          "subject_id" => "MON-cron",
          "agent" => "ops-bot",
          "action" => "silenced"
        })

      body = json_response(conn, 201)
      assert body["subject_type"] == "monitor"
      assert body["subject_id"] == "MON-cron"
    end

    test "creates annotation on error_group subject", %{conn: conn} do
      group = create_error_group("svc", "RuntimeError", 3)

      conn =
        post(conn, "/api/v1/annotations", %{
          "subject_type" => "error_group",
          "subject_id" => group.group_hash,
          "agent" => "fix-bot",
          "action" => "linked",
          "metadata" => %{"pr" => "https://github.com/org/repo/pull/42"}
        })

      body = json_response(conn, 201)
      assert body["subject_type"] == "error_group"
      assert body["subject_id"] == group.group_hash
      assert body["group_hash"] == group.group_hash
    end

    test "returns 404 when subject does not exist", %{conn: conn} do
      conn =
        post(conn, "/api/v1/annotations", %{
          "subject_type" => "target",
          "subject_id" => "TGT-nope",
          "agent" => "bot",
          "action" => "ack"
        })

      body = json_response(conn, 404)
      assert body["code"] == "not_found"
    end

    test "returns 422 for unknown subject_type", %{conn: conn} do
      conn =
        post(conn, "/api/v1/annotations", %{
          "subject_type" => "spaceship",
          "subject_id" => "X-1",
          "agent" => "bot",
          "action" => "ack"
        })

      body = json_response(conn, 422)
      assert body["code"] == "validation_error"
      assert body["errors"]["subject_type"] != nil
    end

    test "returns 422 when subject_type missing", %{conn: conn} do
      conn =
        post(conn, "/api/v1/annotations", %{
          "subject_id" => "X-1",
          "agent" => "bot",
          "action" => "ack"
        })

      body = json_response(conn, 422)
      assert body["errors"]["subject_type"] == ["is required"]
    end

    test "returns 422 when subject_id missing", %{conn: conn} do
      conn =
        post(conn, "/api/v1/annotations", %{
          "subject_type" => "incident",
          "agent" => "bot",
          "action" => "ack"
        })

      body = json_response(conn, 422)
      assert body["errors"]["subject_id"] == ["is required"]
    end

    test "returns 422 when agent missing", %{conn: conn} do
      create_target_with_state("api", "up")

      conn =
        post(conn, "/api/v1/annotations", %{
          "subject_type" => "target",
          "subject_id" => "TGT-api",
          "action" => "ack"
        })

      body = json_response(conn, 422)
      assert body["errors"]["agent"] == ["is required"]
    end
  end

  describe "GET /api/v1/annotations (unified)" do
    test "lists annotations newest-first with summary and cursor envelope", %{conn: conn} do
      create_target_with_state("api", "up")

      post(conn, "/api/v1/annotations", %{
        "subject_type" => "target",
        "subject_id" => "TGT-api",
        "agent" => "alpha",
        "action" => "paged"
      })

      # Sleep 1ms to ensure distinct created_at ordering
      Process.sleep(5)

      post(conn, "/api/v1/annotations", %{
        "subject_type" => "target",
        "subject_id" => "TGT-api",
        "agent" => "beta",
        "action" => "silenced"
      })

      conn = get(conn, "/api/v1/annotations?subject_type=target&subject_id=TGT-api")
      body = json_response(conn, 200)

      assert length(body["annotations"]) == 2
      # Newest-first: beta was posted second
      assert hd(body["annotations"])["agent"] == "beta"
      assert is_binary(body["summary"])
      assert body["summary"] =~ "2 annotations"
      assert body["summary"] =~ "target"
      assert body["summary"] =~ "beta"
      assert body["cursor"] == nil
    end

    test "paginates with cursor when total exceeds limit", %{conn: conn} do
      create_target_with_state("api", "up")

      for i <- 1..3 do
        post(conn, "/api/v1/annotations", %{
          "subject_type" => "target",
          "subject_id" => "TGT-api",
          "agent" => "bot-#{i}",
          "action" => "ping"
        })

        Process.sleep(2)
      end

      conn1 = get(conn, "/api/v1/annotations?subject_type=target&subject_id=TGT-api&limit=2")
      page1 = json_response(conn1, 200)

      assert length(page1["annotations"]) == 2
      assert is_binary(page1["cursor"])
      # Newest-first — bot-3 is newest
      assert Enum.map(page1["annotations"], & &1["agent"]) == ["bot-3", "bot-2"]
      # Summary reflects the TOTAL on the subject, not the page size
      assert page1["summary"] =~ "3 annotations"

      conn2 =
        get(
          conn,
          "/api/v1/annotations?subject_type=target&subject_id=TGT-api&limit=2&cursor=#{page1["cursor"]}"
        )

      page2 = json_response(conn2, 200)
      assert length(page2["annotations"]) == 1
      assert hd(page2["annotations"])["agent"] == "bot-1"
      assert page2["cursor"] == nil
    end

    test "clamps limit above 50 and rejects zero", %{conn: conn} do
      create_target_with_state("api", "up")

      too_big = get(conn, "/api/v1/annotations?subject_type=target&subject_id=TGT-api&limit=51")
      body = json_response(too_big, 422)
      assert body["errors"]["limit"] == ["must be an integer between 1 and 50"]

      zero = get(conn, "/api/v1/annotations?subject_type=target&subject_id=TGT-api&limit=0")
      assert json_response(zero, 422)["code"] == "validation_error"
    end

    test "returns 422 for malformed cursor", %{conn: conn} do
      create_target_with_state("api", "up")

      conn =
        get(conn, "/api/v1/annotations?subject_type=target&subject_id=TGT-api&cursor=not-base64")

      body = json_response(conn, 422)
      assert body["errors"]["cursor"] == ["is invalid"]
    end

    test "returns 404 for nonexistent subject", %{conn: conn} do
      conn = get(conn, "/api/v1/annotations?subject_type=target&subject_id=TGT-nope")
      body = json_response(conn, 404)
      assert body["code"] == "not_found"
    end

    test "returns 422 for unknown subject_type", %{conn: conn} do
      conn = get(conn, "/api/v1/annotations?subject_type=spaceship&subject_id=X-1")
      body = json_response(conn, 422)
      assert body["code"] == "validation_error"
    end
  end

  describe "scope enforcement for unified endpoints" do
    test "POST /api/v1/annotations requires admin scope", %{conn: _conn} do
      {raw_read_key, _} = create_api_key("read-only-key", "read-only")
      conn = build_conn() |> authenticate(raw_read_key)

      conn =
        post(conn, "/api/v1/annotations", %{
          "subject_type" => "incident",
          "subject_id" => "INC-x",
          "agent" => "bot",
          "action" => "ack"
        })

      body = json_response(conn, 403)
      assert body["code"] == "insufficient_scope"
    end

    test "GET /api/v1/annotations works with read scope", %{conn: _conn} do
      create_target_with_state("api", "up")
      {raw_read_key, _} = create_api_key("read-only-key-2", "read-only")
      conn = build_conn() |> authenticate(raw_read_key)

      conn = get(conn, "/api/v1/annotations?subject_type=target&subject_id=TGT-api")
      body = json_response(conn, 200)
      assert body["annotations"] == []
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
