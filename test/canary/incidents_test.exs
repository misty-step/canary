defmodule Canary.IncidentsTest do
  use Canary.DataCase

  alias Canary.{Incidents, Report}
  alias Canary.Schemas.{ErrorGroup, Incident, ServiceEvent, TargetState, Webhook}

  import Canary.Fixtures

  setup do
    clean_status_tables()
    :ets.delete_all_objects(:canary_cooldowns)
    :ok
  end

  describe "correlate/3" do
    test "returns {:ok, nil} when target is up and no open incident exists" do
      create_target_with_state("quiescent", "up")

      assert {:ok, nil} = Incidents.correlate(:health_transition, "TGT-quiescent", "quiescent")
      assert Repo.all(from(i in Incident, where: i.service == "quiescent")) == []
    end

    test "creates an incident for a degraded target and attaches the health signal" do
      create_target_with_state("foo", "degraded")

      assert {:ok, incident} = Incidents.correlate(:health_transition, "TGT-foo", "foo")

      assert incident.service == "foo"
      assert incident.state == "investigating"
      assert incident.severity == "medium"

      assert [%{signal_type: "health_transition", signal_ref: "TGT-foo", resolved_at: nil}] =
               incident.signals
    end

    test "attaches a new error group to the existing open incident" do
      create_target_with_state("foo", "degraded")
      {:ok, _incident} = Incidents.correlate(:health_transition, "TGT-foo", "foo")

      assert {:ok, _result} =
               Canary.Errors.Ingest.ingest(%{
                 "service" => "foo",
                 "error_class" => "TimeoutError",
                 "message" => "timed out"
               })

      incident =
        Repo.one!(
          from(i in Incident,
            preload: [:signals],
            where: i.service == "foo" and i.state == "investigating"
          )
        )

      assert length(incident.signals) == 2
      assert Enum.any?(incident.signals, &(&1.signal_type == "health_transition"))
      assert Enum.any?(incident.signals, &(&1.signal_type == "error_group"))
    end

    test "enforces one open incident per service at the database boundary" do
      create_target_with_state("foo", "degraded")
      {:ok, _incident} = Incidents.correlate(:health_transition, "TGT-foo", "foo")
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      assert {:error, changeset} =
               %Incident{id: Canary.ID.incident_id()}
               |> Incident.changeset(%{
                 service: "foo",
                 state: "investigating",
                 severity: "medium",
                 opened_at: now
               })
               |> Repo.insert()

      assert "has already been taken" in errors_on(changeset).service
    end

    test "escalates severity when three active signals cluster within five minutes" do
      create_target_with_state("foo", "degraded")

      {:ok, _incident} = Incidents.correlate(:health_transition, "TGT-foo", "foo")
      create_error_group("foo", "TimeoutError", 2)
      create_error_group("foo", "RuntimeError", 1)

      {:ok, _incident} =
        Incidents.correlate(:error_group, group_hash("foo", "TimeoutError"), "foo")

      assert {:ok, incident} =
               Incidents.correlate(:error_group, group_hash("foo", "RuntimeError"), "foo")

      assert incident.severity == "high"
      assert length(Enum.filter(incident.signals, &is_nil(&1.resolved_at))) == 3
    end

    test "resolves the incident when all attached signals are no longer active" do
      create_target_with_state("foo", "degraded")
      create_error_group("foo", "TimeoutError", 1)

      {:ok, _incident} = Incidents.correlate(:health_transition, "TGT-foo", "foo")

      {:ok, _incident} =
        Incidents.correlate(:error_group, group_hash("foo", "TimeoutError"), "foo")

      Repo.get!(TargetState, "TGT-foo")
      |> TargetState.changeset(%{
        state: "up",
        last_success_at: DateTime.utc_now() |> DateTime.to_iso8601()
      })
      |> Repo.update!()

      Repo.get!(ErrorGroup, group_hash("foo", "TimeoutError"))
      |> ErrorGroup.changeset(%{status: "resolved"})
      |> Repo.update!()

      assert {:ok, incident} = Incidents.correlate(:health_transition, "TGT-foo", "foo")

      assert incident.state == "resolved"
      assert is_binary(incident.resolved_at)
      assert Enum.all?(incident.signals, &is_binary(&1.resolved_at))
    end

    test "dispatches incident webhook events on create and update" do
      bypass = Bypass.open()
      test_pid = self()
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      %Webhook{id: Canary.ID.webhook_id()}
      |> Webhook.changeset(%{
        url: "http://localhost:#{bypass.port}/hook",
        events: Jason.encode!(["incident.opened", "incident.updated"]),
        secret: "incident-secret",
        created_at: now
      })
      |> Repo.insert!()

      Bypass.expect(bypass, "POST", "/hook", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        send(test_pid, {:webhook_event, Jason.decode!(body)["event"]})
        Plug.Conn.resp(conn, 200, "ok")
      end)

      create_target_with_state("foo", "degraded")
      {:ok, _incident} = Incidents.correlate(:health_transition, "TGT-foo", "foo")
      create_error_group("foo", "TimeoutError", 1)

      {:ok, _incident} =
        Incidents.correlate(:error_group, group_hash("foo", "TimeoutError"), "foo")

      assert_receive {:webhook_event, "incident.opened"}
      assert_receive {:webhook_event, "incident.updated"}

      assert ["incident.opened", "incident.updated"] ==
               Repo.all(
                 from(e in ServiceEvent,
                   where: e.service == "foo" and e.entity_type == "incident",
                   order_by: [asc: e.created_at, asc: e.id],
                   select: e.event
                 )
               )
    end
  end

  describe "report integration" do
    test "includes correlated incidents in the unified report" do
      create_target_with_state("foo", "degraded")
      create_error_group("foo", "TimeoutError", 1)

      {:ok, _incident} = Incidents.correlate(:health_transition, "TGT-foo", "foo")

      {:ok, _incident} =
        Incidents.correlate(:error_group, group_hash("foo", "TimeoutError"), "foo")

      assert {:ok, report} = Report.generate(window: "1h")
      assert [%{service: "foo", signal_count: 2}] = report.incidents
    end

    test "omits stale error-only incidents from the unified report" do
      create_error_group("foo", "TimeoutError", 1)
      hash = group_hash("foo", "TimeoutError")

      {:ok, _incident} = Incidents.correlate(:error_group, hash, "foo")

      stale = DateTime.utc_now() |> DateTime.add(-10 * 60, :second) |> DateTime.to_iso8601()

      Repo.get!(ErrorGroup, hash)
      |> ErrorGroup.changeset(%{last_seen_at: stale})
      |> Repo.update!()

      assert {:ok, report} = Report.generate(window: "1h")
      assert report.incidents == []
    end
  end

  defp group_hash(service, error_class) do
    :crypto.hash(:sha256, "#{service}:#{error_class}") |> Base.encode16(case: :lower)
  end
end
