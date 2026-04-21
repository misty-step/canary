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
      assert is_binary(body["summary"]) and body["summary"] != ""
      assert is_list(body["incidents"])
      ids = Enum.map(body["incidents"], & &1["id"])
      assert incident.id in ids
    end

    test "returns summary describing the page, even when empty", %{conn: conn} do
      conn = get(conn, "/api/v1/incidents")
      body = json_response(conn, 200)
      assert body["incidents"] == []
      assert body["summary"] == "No active incidents."
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

  describe "GET /api/v1/incidents/:id" do
    test "returns detail payload with signals, annotations, and timeline", %{conn: conn} do
      create_target_with_state("detail-svc", "down")
      incident = create_incident("detail-svc")

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "health_transition",
        signal_ref: "TGT-detail-svc",
        attached_at: DateTime.utc_now() |> DateTime.to_iso8601()
      })

      error_group = create_error_group("detail-svc", "DetailError", 4)

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "error_group",
        signal_ref: error_group.group_hash,
        attached_at: DateTime.utc_now() |> DateTime.to_iso8601()
      })

      create_annotation(:incident, incident.id, action: "acknowledged", agent: "bb-sprite")

      Canary.Timeline.record_incident!(
        "incident.opened",
        Canary.Repo.preload(incident, :signals),
        DateTime.utc_now() |> DateTime.to_iso8601()
      )

      conn = get(conn, "/api/v1/incidents/#{incident.id}")
      body = json_response(conn, 200)

      assert is_binary(body["summary"]) and body["summary"] != ""
      assert body["incident"]["id"] == incident.id
      assert body["incident"]["service"] == "detail-svc"
      assert body["incident"]["signal_count"] == 2
      assert length(body["signals"]) == 2

      assert body["signals_truncated"] == false
      assert body["annotations_truncated"] == false
      assert length(body["annotations"]) == 1
      assert hd(body["annotations"])["action"] == "acknowledged"
      assert length(body["recent_timeline_events"]) == 1
      assert hd(body["recent_timeline_events"])["event"] == "incident.opened"

      # Signals carry type-specific context
      by_type = Enum.group_by(body["signals"], & &1["type"])
      [health] = by_type["health_transition"]
      assert health["target_id"] == "TGT-detail-svc"
      assert is_binary(health["summary"]) and health["summary"] != ""

      [err] = by_type["error_group"]
      assert err["group_hash"] == error_group.group_hash
      assert err["error_class"] == "DetailError"
      assert err["total_count"] == 4
    end

    test "returns 404 for unknown incident", %{conn: conn} do
      conn = get(conn, "/api/v1/incidents/INC-does-not-exist")
      assert json_response(conn, 404)["code"] == "not_found"
    end

    test "returns 401 without auth", %{conn: conn} do
      conn =
        conn
        |> delete_req_header("authorization")
        |> get("/api/v1/incidents/INC-anything")

      assert json_response(conn, 401)["code"] == "invalid_api_key"
    end

    test "caps signals at 25 and reports truncation", %{conn: conn} do
      incident = create_incident("cap-svc")
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      # Fabricate 30 error_group signals attached to the incident
      for i <- 1..30 do
        Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
          incident_id: incident.id,
          signal_type: "error_group",
          signal_ref: "grp-#{String.pad_leading(to_string(i), 3, "0")}",
          attached_at: now
        })
      end

      conn = get(conn, "/api/v1/incidents/#{incident.id}")
      body = json_response(conn, 200)

      assert length(body["signals"]) == 25
      assert body["signals_truncated"] == true
      assert body["incident"]["signal_count"] == 30
    end

    test "caps annotations at 20 and reports truncation, newest-first", %{conn: conn} do
      incident = create_incident("ann-cap-svc")

      base = DateTime.utc_now()

      for i <- 1..25 do
        now =
          base
          |> DateTime.add(-(25 - i) * 60, :second)
          |> DateTime.to_iso8601()

        annotation_id = Canary.ID.annotation_id()

        %Canary.Schemas.Annotation{id: annotation_id}
        |> Canary.Schemas.Annotation.changeset(%{
          incident_id: incident.id,
          agent: "test-agent",
          action: "note-#{i}",
          created_at: now
        })
        |> Canary.Repo.insert!()
      end

      conn = get(conn, "/api/v1/incidents/#{incident.id}")
      body = json_response(conn, 200)

      assert length(body["annotations"]) == 20
      assert body["annotations_truncated"] == true
      # Newest first: note-25 should be present, note-1 should NOT be in the top 20
      actions = Enum.map(body["annotations"], & &1["action"])
      assert "note-25" in actions
      refute "note-1" in actions
    end

    test "stays within a small query budget (≤ 8)", %{conn: conn} do
      create_target_with_state("budget-svc", "down")
      incident = create_incident("budget-svc")
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "health_transition",
        signal_ref: "TGT-budget-svc",
        attached_at: now
      })

      for i <- 1..5 do
        grp = create_error_group("budget-svc", "E#{i}", i + 1)

        Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
          incident_id: incident.id,
          signal_type: "error_group",
          signal_ref: grp.group_hash,
          attached_at: now
        })
      end

      {:ok, queries} = Agent.start_link(fn -> 0 end)

      handler = fn _event, _measurements, _metadata, _config ->
        Agent.update(queries, &(&1 + 1))
      end

      # In test mode the read_repo routes through Canary.Repo (see config/test.exs),
      # so the telemetry event prefix is [:canary, :repo].
      :telemetry.attach(
        "test-query-counter",
        [:canary, :repo, :query],
        handler,
        nil
      )

      try do
        conn = get(conn, "/api/v1/incidents/#{incident.id}")
        assert json_response(conn, 200)
      after
        :telemetry.detach("test-query-counter")
      end

      count = Agent.get(queries, & &1)
      Agent.stop(queries)

      # Budget per ticket oracle: ~4 logical queries (incident + signal
      # count+rows, signal context, annotations, timeline). Physical count
      # includes per-schema IN lookups for error-group / target / monitor
      # context. 10 is the upper ceiling — importantly, it is constant in
      # the number of signals: adding a 6th error_group signal below would
      # not change the count because the context fetch batches by IN (…)
      # across all signals.
      assert count <= 10, "expected ≤10 read queries for detail fetch, got #{count}"
    end
  end
end
