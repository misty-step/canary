defmodule Canary.MetricsTest do
  use Canary.DataCase

  import Canary.Fixtures

  alias Canary.Alerter.CircuitBreaker
  alias Canary.Errors.Ingest
  alias Canary.Health.Checker
  alias Canary.Schemas.{Target, TargetState, Webhook}
  alias Canary.Workers.WebhookDelivery

  @valid_ingest %{
    "service" => "metrics-svc",
    "error_class" => "RuntimeError",
    "message" => "metrics test failure"
  }

  setup do
    clean_status_tables()
    :ets.delete_all_objects(:canary_dedup_cache)
    :ets.delete_all_objects(:canary_circuit_breakers)
    :ets.delete_all_objects(:canary_cooldowns)

    previous = Application.get_env(:canary, :allow_private_targets, false)
    Application.put_env(:canary, :allow_private_targets, true)
    on_exit(fn -> Application.put_env(:canary, :allow_private_targets, previous) end)

    :ok
  end

  test "ingest metrics capture totals, durations, and errors" do
    before = scrape()

    assert {:ok, _result} = Ingest.ingest(@valid_ingest)
    assert {:error, :validation_error, _errors} = Ingest.ingest(%{"service" => "metrics-svc"})

    after_scrape = scrape()

    assert counter_delta(before, after_scrape, "canary_ingest_total") == 2.0
    assert counter_delta(before, after_scrape, "canary_ingest_errors_total") == 1.0
    assert counter_delta(before, after_scrape, "canary_ingest_duration_seconds_count") == 2.0
  end

  test "probe metrics expose totals, durations, and per-target state gauges" do
    bypass = Bypass.open()

    Bypass.expect_once(bypass, "GET", "/healthz", fn conn ->
      Plug.Conn.resp(conn, 500, "nope")
    end)

    now = DateTime.utc_now() |> DateTime.to_iso8601()

    target =
      Repo.insert!(%Target{
        id: "TGT-metrics-probe",
        name: "metrics-probe",
        service: "metrics-probe",
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
      before = scrape()
      Checker.check_now(target.id)

      eventually(fn ->
        assert Repo.get!(TargetState, target.id).state == "degraded"
      end)

      after_scrape = scrape()

      assert counter_delta(before, after_scrape, "canary_probe_total", %{"result" => "failure"}) ==
               1.0

      assert counter_delta(
               before,
               after_scrape,
               "canary_probe_duration_seconds_count",
               %{"result" => "failure"}
             ) == 1.0

      assert sample_value(
               after_scrape,
               "canary_probe_state",
               %{"target_id" => target.id, "state" => "degraded"}
             ) == 1.0

      assert sample_value(
               after_scrape,
               "canary_probe_state",
               %{"target_id" => target.id, "state" => "up"}
             ) == 0.0
    after
      GenServer.stop(pid)
    end
  end

  test "probe metrics capture healthy checks" do
    bypass = Bypass.open()

    Bypass.expect_once(bypass, "GET", "/healthz", fn conn ->
      Plug.Conn.resp(conn, 200, "ok")
    end)

    now = DateTime.utc_now() |> DateTime.to_iso8601()

    target =
      Repo.insert!(%Target{
        id: "TGT-metrics-probe-ok",
        name: "metrics-probe-ok",
        service: "metrics-probe-ok",
        url: "http://localhost:#{bypass.port}/healthz",
        created_at: now,
        degraded_after: 1,
        down_after: 3,
        up_after: 1
      })

    Repo.insert!(%TargetState{
      target_id: target.id,
      state: "unknown",
      consecutive_failures: 0,
      consecutive_successes: 0,
      last_checked_at: now
    })

    {:ok, pid} = Checker.start_link(target)

    try do
      before = scrape()
      Checker.check_now(target.id)

      eventually(fn ->
        assert Repo.get!(TargetState, target.id).state == "up"
      end)

      after_scrape = scrape()

      assert counter_delta(before, after_scrape, "canary_probe_total", %{"result" => "success"}) ==
               1.0

      assert counter_delta(
               before,
               after_scrape,
               "canary_probe_duration_seconds_count",
               %{"result" => "success"}
             ) == 1.0
    after
      GenServer.stop(pid)
    end
  end

  test "webhook, circuit breaker, queue depth, and Oban metrics are exposed" do
    bypass = Bypass.open()
    oban_bypass = Bypass.open()
    webhook_id = Canary.ID.webhook_id()
    oban_webhook_id = Canary.ID.webhook_id()
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    Repo.insert!(
      %Webhook{id: webhook_id}
      |> Webhook.changeset(%{
        url: "http://localhost:#{bypass.port}/hook",
        events: Jason.encode!(["error.new_class"]),
        secret: "metrics-secret",
        created_at: now
      })
    )

    Repo.insert!(
      %Webhook{id: oban_webhook_id}
      |> Webhook.changeset(%{
        url: "http://localhost:#{oban_bypass.port}/hook",
        events: Jason.encode!(["error.new_class"]),
        secret: "metrics-secret",
        created_at: now
      })
    )

    before = scrape()
    delivery_id = Canary.ID.generate("DLV")

    Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
      Plug.Conn.resp(conn, 200, "ok")
    end)

    assert :ok =
             WebhookDelivery.perform(%Oban.Job{
               args: %{
                 "webhook_id" => webhook_id,
                 "payload" => %{"event" => "error.new_class"},
                 "event" => "error.new_class",
                 "delivery_id" => delivery_id
               }
             })

    missing_delivery_id = Canary.ID.generate("DLV")

    assert :ok =
             WebhookDelivery.perform(%Oban.Job{
               args: %{
                 "webhook_id" => "WHK-missing",
                 "payload" => %{},
                 "event" => "error.new_class",
                 "delivery_id" => missing_delivery_id
               }
             })

    for _ <- 1..10, do: CircuitBreaker.record_failure(webhook_id)

    suppressed_delivery_id = Canary.ID.generate("DLV")

    assert :ok =
             WebhookDelivery.perform(%Oban.Job{
               args: %{
                 "webhook_id" => webhook_id,
                 "payload" => %{},
                 "event" => "error.new_class",
                 "delivery_id" => suppressed_delivery_id
               }
             })

    Bypass.expect_once(oban_bypass, "POST", "/hook", fn conn ->
      Plug.Conn.resp(conn, 200, "ok")
    end)

    WebhookDelivery.enqueue_for_event("error.new_class", %{
      "event" => "error.new_class",
      "error" => %{"group_hash" => "grp-metrics-oban"}
    })

    after_scrape =
      eventually_value(fn ->
        body = scrape()

        delivered =
          counter_delta(before, body, "canary_webhook_delivery_total", %{"status" => "delivered"})

        if delivered >= 1.0 do
          body
        end
      end)

    assert counter_delta(
             before,
             after_scrape,
             "canary_webhook_delivery_total",
             %{"status" => "delivered"}
           ) >= 1.0

    assert counter_delta(
             before,
             after_scrape,
             "canary_webhook_delivery_total",
             %{"status" => "discarded"}
           ) >= 1.0

    assert counter_delta(
             before,
             after_scrape,
             "canary_webhook_delivery_total",
             %{"status" => "suppressed"}
           ) >= 1.0

    assert sample_value(after_scrape, "canary_webhook_queue_depth") == 0.0

    assert sample_value(after_scrape, "canary_circuit_breaker_state", %{
             "webhook_id" => webhook_id
           }) == 1.0

    assert sample_value(
             after_scrape,
             "canary_oban_queue_depth",
             %{"queue" => "webhooks"},
             -1.0
           ) == 0.0

    assert counter_delta(
             before,
             after_scrape,
             "canary_oban_job_duration_seconds_count",
             %{"queue" => "webhooks", "state" => "success"}
           ) >= 1.0
  end

  defp scrape do
    Canary.Metrics.emit_runtime_metrics()
    Canary.Metrics.scrape()
  end

  defp counter_delta(before, after_scrape, metric, labels \\ %{}) do
    sample_value(after_scrape, metric, labels, 0.0) - sample_value(before, metric, labels, 0.0)
  end

  defp sample_value(body, metric, labels \\ %{}, default \\ nil) do
    body
    |> String.split("\n", trim: true)
    |> Enum.find_value(default, fn line ->
      with {:ok, name, parsed_labels, value} <- parse_sample(line),
           true <- name == metric,
           true <-
             Enum.all?(labels, fn {key, expected} -> Map.get(parsed_labels, key) == expected end) do
        value
      else
        _ -> nil
      end
    end)
  end

  defp parse_sample("#" <> _comment), do: :error

  defp parse_sample(line) do
    case Regex.run(~r/^([a-zA-Z_:][a-zA-Z0-9_:]*)(?:\{([^}]*)\})?\s+([-+eE0-9.]+)$/, line) do
      [_, name, label_string, value] ->
        {:ok, name, parse_labels(label_string), parse_number(value)}

      [_, name, value] ->
        {:ok, name, %{}, parse_number(value)}

      _ ->
        :error
    end
  end

  defp parse_labels(""), do: %{}

  defp parse_labels(label_string) do
    label_string
    |> String.split(",", trim: true)
    |> Enum.map(fn segment ->
      [key, value] = String.split(segment, "=", parts: 2)
      {key, String.trim(value, "\"")}
    end)
    |> Map.new()
  end

  defp parse_number(value) do
    case Float.parse(value) do
      {number, ""} ->
        number

      _ ->
        case Integer.parse(value) do
          {number, ""} -> number * 1.0
          _ -> raise ArgumentError, "unsupported metric sample value: #{inspect(value)}"
        end
    end
  end

  defp eventually(fun, attempts \\ 20)

  defp eventually(fun, attempts) when attempts > 0 do
    try do
      fun.()
    rescue
      ExUnit.AssertionError ->
        Process.sleep(25)
        eventually(fun, attempts - 1)
    end
  end

  defp eventually(_fun, 0), do: flunk("condition not met in time")

  defp eventually_value(fun, attempts \\ 20)

  defp eventually_value(fun, attempts) when attempts > 0 do
    case fun.() do
      nil ->
        Process.sleep(25)
        eventually_value(fun, attempts - 1)

      value ->
        value
    end
  end

  defp eventually_value(_fun, 0), do: flunk("condition not met in time")
end
