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

      insert_error_for_group(error_group,
        classification_category: "application",
        classification_persistence: "persistent",
        classification_component: "runtime"
      )

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

      assert err["classification"] == %{
               "category" => "application",
               "persistence" => "persistent",
               "component" => "runtime"
             }

      # Per-signal annotation_count is 0 when no annotations target the signal's
      # underlying error_group / target / monitor subject (only an incident
      # annotation exists in this setup).
      assert err["annotation_count"] == 0
      assert health["annotation_count"] == 0
    end

    test "surfaces annotation_count per signal from the underlying subject (error_group, target, monitor)",
         %{conn: conn} do
      create_target_with_state("ramp-api", "down")
      create_monitor_with_state("ramp-cron", "down")
      error_group = create_error_group("ramp-api", "RampError", 2)

      incident = create_incident("ramp-api")
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "error_group",
        signal_ref: error_group.group_hash,
        attached_at: now
      })

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "health_transition",
        signal_ref: "TGT-ramp-api",
        attached_at: now
      })

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "health_transition",
        signal_ref: "MON-ramp-cron",
        attached_at: now
      })

      # 3 annotations on the error_group, 1 on the target, 0 on the monitor.
      for agent <- ["a", "b", "c"] do
        create_annotation(:group, error_group.group_hash, agent: agent)
      end

      create_annotation(:target, "TGT-ramp-api", agent: "ops-bot")

      conn = get(conn, "/api/v1/incidents/#{incident.id}")
      body = json_response(conn, 200)

      by_ref =
        body["signals"]
        |> Enum.group_by(fn
          %{"type" => "error_group", "group_hash" => h} -> {"error_group", h}
          %{"type" => "health_transition", "target_id" => t} when is_binary(t) -> {"target", t}
          %{"type" => "health_transition", "monitor_id" => m} when is_binary(m) -> {"monitor", m}
          sig -> {:unknown, sig}
        end)

      [err] = by_ref[{"error_group", error_group.group_hash}]
      [tgt] = by_ref[{"target", "TGT-ramp-api"}]
      [mon] = by_ref[{"monitor", "MON-ramp-cron"}]

      assert err["annotation_count"] == 3
      assert tgt["annotation_count"] == 1
      assert mon["annotation_count"] == 0
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

    test "returns an action brief over existing incident state", %{conn: conn} do
      incident = create_incident("brief-svc")
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      active_group = create_error_group("brief-svc", "EmbeddingError", 7)
      resolved_group = create_error_group("brief-svc", "ConfigError", 4)

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "error_group",
        signal_ref: active_group.group_hash,
        attached_at: now
      })

      Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
        incident_id: incident.id,
        signal_type: "error_group",
        signal_ref: resolved_group.group_hash,
        attached_at: now,
        resolved_at: now
      })

      create_annotation(:incident, incident.id,
        action: "fixed",
        agent: "codex",
        metadata: %{deployment: "https://example.com/deploy"}
      )

      conn = get(conn, "/api/v1/incidents/#{incident.id}")
      body = json_response(conn, 200)["action_brief"]

      assert body["summary"] =~ "brief-svc action brief"
      assert body["recommendation"]["action"] == "triage"
      assert body["recommendation"]["reason"] =~ "active signal"

      assert body["signal_counts"] == %{
               "active" => 1,
               "resolved" => 1,
               "total" => 2,
               "visible" => 2
             }

      assert body["signals_truncated"] == false
      assert body["latest_annotation"]["action"] == "fixed"
      assert body["latest_annotation"]["agent"] == "codex"
      refute Map.has_key?(body, "active_signals")
      refute Map.has_key?(body, "resolved_signals")
      refute Map.has_key?(body, "incident")
      refute Map.has_key?(body, "action_state")
      refute Map.has_key?(body, "suggested_annotation_actions")
    end

    test "recommends watch when all visible active signals already have annotations", %{
      conn: conn
    } do
      incident = create_incident("watch-svc")
      now = DateTime.utc_now() |> DateTime.to_iso8601()
      group = create_error_group("watch-svc", "WatchError", 2)

      insert_incident_signal(incident, "error_group", group.group_hash, attached_at: now)
      create_annotation(:group, group.group_hash, action: "triaged", agent: "bb-sprite")

      conn = get(conn, "/api/v1/incidents/#{incident.id}")
      body = json_response(conn, 200)["action_brief"]

      assert body["recommendation"]["action"] == "watch"
      assert body["recommendation"]["reason"] =~ "already have coordination annotations"

      assert body["signal_counts"] == %{
               "active" => 1,
               "resolved" => 0,
               "total" => 1,
               "visible" => 1
             }
    end

    test "recommends recovery verification when no active signals remain", %{conn: conn} do
      incident = create_incident("resolved-svc")
      now = DateTime.utc_now() |> DateTime.to_iso8601()
      group = create_error_group("resolved-svc", "ResolvedError", 2)

      insert_incident_signal(incident, "error_group", group.group_hash,
        attached_at: now,
        resolved_at: now
      )

      conn = get(conn, "/api/v1/incidents/#{incident.id}")
      body = json_response(conn, 200)["action_brief"]

      assert body["recommendation"]["action"] == "verify-recovery"
      assert body["recommendation"]["reason"] =~ "No active signals remain"

      assert body["signal_counts"] == %{
               "active" => 0,
               "resolved" => 1,
               "total" => 1,
               "visible" => 1
             }
    end

    test "marks action brief as truncated before recommending action", %{conn: conn} do
      incident = create_incident("truncated-svc")
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      for i <- 1..30 do
        insert_incident_signal(
          incident,
          "error_group",
          "grp-#{String.pad_leading(to_string(i), 3, "0")}",
          attached_at: now
        )
      end

      conn = get(conn, "/api/v1/incidents/#{incident.id}")
      body = json_response(conn, 200)["action_brief"]

      assert body["signals_truncated"] == true
      assert body["recommendation"]["action"] == "inspect-truncated-signals"
      assert body["recommendation"]["reason"] =~ "complete recommendation cannot be derived"

      assert body["signal_counts"] == %{
               "active" => 25,
               "resolved" => 0,
               "total" => 30,
               "visible" => 25
             }
    end

    test "keeps truncated recommendation conservative when visible signals are resolved", %{
      conn: conn
    } do
      incident = create_incident("truncated-resolved-svc")
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      for i <- 1..30 do
        insert_incident_signal(
          incident,
          "error_group",
          "grp-resolved-#{String.pad_leading(to_string(i), 3, "0")}",
          attached_at: now,
          resolved_at: now
        )
      end

      conn = get(conn, "/api/v1/incidents/#{incident.id}")
      body = json_response(conn, 200)["action_brief"]

      assert body["signals_truncated"] == true
      assert body["recommendation"]["action"] == "inspect-truncated-signals"

      assert body["signal_counts"] == %{
               "active" => 0,
               "resolved" => 25,
               "total" => 30,
               "visible" => 25
             }
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
          subject_type: "incident",
          subject_id: incident.id,
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

    test "stays within a small query budget (≤ 10) that is constant in the number of signals", %{
      conn: conn
    } do
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

  defp insert_incident_signal(incident, signal_type, signal_ref, attrs) do
    Canary.Repo.insert!(%Canary.Schemas.IncidentSignal{
      incident_id: incident.id,
      signal_type: signal_type,
      signal_ref: signal_ref,
      attached_at: Keyword.fetch!(attrs, :attached_at),
      resolved_at: Keyword.get(attrs, :resolved_at)
    })
  end

  defp insert_error_for_group(error_group, attrs) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    %Canary.Schemas.Error{id: error_group.last_error_id}
    |> Canary.Schemas.Error.changeset(%{
      service: error_group.service,
      error_class: error_group.error_class,
      message: "classified detail error",
      group_hash: error_group.group_hash,
      created_at: now,
      classification_category: Keyword.fetch!(attrs, :classification_category),
      classification_persistence: Keyword.fetch!(attrs, :classification_persistence),
      classification_component: Keyword.fetch!(attrs, :classification_component)
    })
    |> Canary.Repo.insert!()
  end
end
