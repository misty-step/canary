defmodule Canary.MonitorsTest do
  use Canary.DataCase

  import Ecto.Query

  alias Canary.Monitors
  alias Canary.Schemas.{MonitorCheckIn, MonitorState, ServiceEvent}
  import Canary.Fixtures

  setup do
    clean_status_tables()
    :ok
  end

  test "stores alive check-ins and updates monitor state" do
    {:ok, monitor} =
      Monitors.add_monitor(%{
        "name" => "desktop-active-timer",
        "mode" => "ttl",
        "expected_every_ms" => 90_000
      })

    observed_at = DateTime.utc_now() |> DateTime.to_iso8601()

    assert {:ok, %{check_in: check_in, state: state}} =
             Monitors.process_check_in(%{
               "monitor" => "desktop-active-timer",
               "status" => "alive",
               "observed_at" => observed_at,
               "summary" => "timer still running"
             })

    assert check_in.monitor_id == monitor.id
    assert Repo.aggregate(MonitorCheckIn, :count) == 1
    assert state.state == "up"
    assert state.last_check_in_status == "alive"
    assert state.last_check_in_at == observed_at
    assert state.sequence == 1
  end

  test "degrades, goes down, and recovers overdue monitors" do
    {:ok, monitor} =
      Monitors.add_monitor(%{
        "name" => "nightly-import",
        "mode" => "schedule",
        "expected_every_ms" => 60_000
      })

    base = DateTime.utc_now() |> DateTime.add(-180, :second)

    assert {:ok, _result} =
             Monitors.process_check_in(%{
               "monitor" => "nightly-import",
               "status" => "ok",
               "observed_at" => DateTime.to_iso8601(base)
             })

    :ok = Monitors.evaluate_overdue(at: DateTime.add(base, 61, :second))
    assert Repo.get!(MonitorState, monitor.id).state == "degraded"

    :ok = Monitors.evaluate_overdue(at: DateTime.add(base, 122, :second))
    assert Repo.get!(MonitorState, monitor.id).state == "down"

    assert {:ok, _result} =
             Monitors.process_check_in(%{
               "monitor" => "nightly-import",
               "status" => "ok",
               "observed_at" => DateTime.to_iso8601(DateTime.add(base, 123, :second))
             })

    assert Repo.get!(MonitorState, monitor.id).state == "up"

    events =
      from(e in ServiceEvent,
        where: e.entity_ref == ^monitor.id,
        order_by: [asc: e.created_at]
      )
      |> Repo.all()

    assert Enum.map(events, & &1.event) == [
             "health_check.recovered",
             "health_check.degraded",
             "health_check.down",
             "health_check.recovered"
           ]

    payload = events |> Enum.at(1) |> Map.fetch!(:payload) |> Jason.decode!()
    assert payload["monitor"]["name"] == "nightly-import"
  end
end
