defmodule Canary.Health.CheckerTest do
  use Canary.DataCase

  import Canary.Fixtures

  alias Canary.Health.Checker
  alias Canary.Schemas.{ServiceEvent, Target, TargetState}

  setup do
    clean_status_tables()
    previous = Application.get_env(:canary, :allow_private_targets, false)
    Application.put_env(:canary, :allow_private_targets, true)
    on_exit(fn -> Application.put_env(:canary, :allow_private_targets, previous) end)
    :ok
  end

  test "uses a permanent child restart policy" do
    target = %Target{
      id: "TGT-restart",
      name: "restart",
      service: "restart",
      url: "https://example.com"
    }

    assert %{restart: :permanent} = Checker.child_spec(target)
  end

  test "records health transition timeline events using target service identity" do
    bypass = Bypass.open()

    Bypass.expect_once(bypass, "GET", "/healthz", fn conn ->
      Plug.Conn.resp(conn, 500, "nope")
    end)

    now = DateTime.utc_now() |> DateTime.to_iso8601()

    target =
      Repo.insert!(%Target{
        id: "TGT-api-web",
        name: "api-web",
        service: "api",
        url: "http://localhost:#{bypass.port}/healthz",
        created_at: now,
        degraded_after: 1,
        down_after: 3,
        up_after: 1
      })

    Repo.insert!(%TargetState{
      target_id: target.id,
      state: "up",
      consecutive_failures: 0,
      consecutive_successes: 1,
      last_checked_at: now,
      last_success_at: now
    })

    {:ok, pid} = Checker.start_link(target)

    try do
      Checker.check_now(target.id)

      event =
        eventually(fn ->
          Repo.one(
            from(e in ServiceEvent,
              where: e.event == "health_check.degraded" and e.entity_ref == ^target.id
            )
          )
        end)

      payload = Jason.decode!(event.payload)

      assert event.service == "api"
      assert payload["target"]["service"] == "api"
      assert payload["target"]["name"] == "api-web"
      assert payload["state"] == "degraded"
    after
      GenServer.stop(pid)
    end
  end

  test "falls back to target name when legacy targets have no service" do
    bypass = Bypass.open()

    Bypass.expect_once(bypass, "GET", "/healthz", fn conn ->
      Plug.Conn.resp(conn, 500, "nope")
    end)

    now = DateTime.utc_now() |> DateTime.to_iso8601()

    target =
      Repo.insert!(%Target{
        id: "TGT-legacy-api",
        name: "legacy-api",
        service: nil,
        url: "http://localhost:#{bypass.port}/healthz",
        created_at: now,
        degraded_after: 1,
        down_after: 3,
        up_after: 1
      })

    Repo.insert!(%TargetState{
      target_id: target.id,
      state: "up",
      consecutive_failures: 0,
      consecutive_successes: 1,
      last_checked_at: now,
      last_success_at: now
    })

    {:ok, pid} = Checker.start_link(target)

    try do
      Checker.check_now(target.id)

      event =
        eventually(fn ->
          Repo.one(
            from(e in ServiceEvent,
              where: e.event == "health_check.degraded" and e.entity_ref == ^target.id
            )
          )
        end)

      payload = Jason.decode!(event.payload)

      assert event.service == "legacy-api"
      assert payload["target"]["service"] == "legacy-api"
    after
      GenServer.stop(pid)
    end
  end

  defp eventually(fun, attempts \\ 20)

  defp eventually(fun, attempts) when attempts > 0 do
    case fun.() do
      nil ->
        Process.sleep(25)
        eventually(fun, attempts - 1)

      value ->
        value
    end
  end

  defp eventually(_fun, 0), do: flunk("condition not met in time")
end
