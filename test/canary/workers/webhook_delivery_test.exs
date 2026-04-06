defmodule Canary.Workers.WebhookDeliveryTest do
  use Canary.DataCase

  alias Canary.WebhookDeliveries
  alias Canary.Workers.WebhookDelivery
  alias Canary.Alerter.{CircuitBreaker, Signer}
  alias Canary.Schemas.Webhook

  setup do
    bypass = Bypass.open()

    webhook_id = Canary.ID.webhook_id()
    secret = "test-webhook-secret"
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    %Webhook{id: webhook_id}
    |> Webhook.changeset(%{
      url: "http://localhost:#{bypass.port}/hook",
      events: Jason.encode!(["error.new_class", "health_check.state_change"]),
      secret: secret,
      created_at: now
    })
    |> Repo.insert!()

    :ets.delete_all_objects(:canary_circuit_breakers)
    :ets.delete_all_objects(:canary_cooldowns)

    on_exit(fn ->
      :ets.delete_all_objects(:canary_circuit_breakers)
      :ets.delete_all_objects(:canary_cooldowns)
    end)

    %{bypass: bypass, webhook_id: webhook_id, secret: secret}
  end

  describe "perform/1" do
    test "delivers with HMAC signature", %{bypass: bypass, webhook_id: wid, secret: secret} do
      payload = %{"event" => "error.new_class", "data" => %{"id" => "ERR-test"}}
      delivery_id = Canary.ID.generate("DLV")

      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        [signature] = Plug.Conn.get_req_header(conn, "x-signature")
        [event] = Plug.Conn.get_req_header(conn, "x-event")
        [header_delivery_id] = Plug.Conn.get_req_header(conn, "x-delivery-id")

        assert String.starts_with?(signature, "sha256=")
        assert Signer.verify(body, secret, signature)
        assert event == "error.new_class"
        assert header_delivery_id == delivery_id

        Plug.Conn.resp(conn, 200, "ok")
      end)

      job = %Oban.Job{
        args: %{
          "webhook_id" => wid,
          "payload" => payload,
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        }
      }

      assert :ok = WebhookDelivery.perform(job)

      [row] = WebhookDeliveries.list(webhook_id: wid, event: "error.new_class")
      assert row.delivery_id == delivery_id
      assert row.status == "delivered"
      assert row.attempt_count == 1
      assert is_binary(row.first_attempt_at)
      assert is_binary(row.last_attempt_at)
      assert is_binary(row.delivered_at)
      assert is_nil(row.reason)
    end

    test "uses a stable delivery id across retry attempts", %{bypass: bypass, webhook_id: wid} do
      payload = %{"event" => "error.new_class", "error" => %{"group_hash" => "grp-stable"}}
      delivery_id = Canary.ID.generate("DLV")
      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        [header_delivery_id] = Plug.Conn.get_req_header(conn, "x-delivery-id")
        send(test_pid, {:delivery_id, header_delivery_id})
        Plug.Conn.resp(conn, 500, "Internal Server Error")
      end)

      first = %Oban.Job{
        attempt: 1,
        max_attempts: 4,
        args: %{
          "webhook_id" => wid,
          "payload" => payload,
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        }
      }

      second = %Oban.Job{
        attempt: 2,
        max_attempts: 4,
        args: %{
          "webhook_id" => wid,
          "payload" => payload,
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        }
      }

      assert {:error, "HTTP 500"} = WebhookDelivery.perform(first)
      assert {:error, "HTTP 500"} = WebhookDelivery.perform(second)

      assert_receive {:delivery_id, ^delivery_id}
      assert_receive {:delivery_id, ^delivery_id}

      [row] = WebhookDeliveries.list(delivery_id: delivery_id)
      assert row.status == "retrying"
      assert row.attempt_count == 2
    end

    test "derives a stable delivery id for legacy jobs without delivery_id", %{
      bypass: bypass,
      webhook_id: wid
    } do
      payload = %{"event" => "error.new_class", "error" => %{"group_hash" => "grp-legacy"}}
      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        [header_delivery_id] = Plug.Conn.get_req_header(conn, "x-delivery-id")
        send(test_pid, {:legacy_delivery_id, header_delivery_id})
        Plug.Conn.resp(conn, 500, "Internal Server Error")
      end)

      first = %Oban.Job{
        id: 42,
        attempt: 1,
        max_attempts: 4,
        args: %{"webhook_id" => wid, "payload" => payload, "event" => "error.new_class"}
      }

      second = %{first | attempt: 2}

      assert {:error, "HTTP 500"} = WebhookDelivery.perform(first)
      assert {:error, "HTTP 500"} = WebhookDelivery.perform(second)

      assert_receive {:legacy_delivery_id, legacy_delivery_id}
      assert_receive {:legacy_delivery_id, ^legacy_delivery_id}

      [row] = WebhookDeliveries.list(delivery_id: legacy_delivery_id)
      assert row.attempt_count == 2
      assert row.status == "retrying"
    end

    test "skips when circuit breaker is open", %{bypass: bypass, webhook_id: wid} do
      for _ <- 1..10, do: CircuitBreaker.record_failure(wid)
      assert CircuitBreaker.open?(wid)
      delivery_id = Canary.ID.generate("DLV")

      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      job = %Oban.Job{
        args: %{
          "webhook_id" => wid,
          "payload" => %{},
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        }
      }

      assert :ok = WebhookDelivery.perform(job)
      refute_received :webhook_delivered

      [row] = WebhookDeliveries.list(delivery_id: delivery_id)
      assert row.status == "suppressed"
      assert row.reason == "circuit_open"
      assert row.attempt_count == 0
    end

    test "returns error on non-2xx response", %{bypass: bypass, webhook_id: wid} do
      delivery_id = Canary.ID.generate("DLV")

      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        Plug.Conn.resp(conn, 500, "Internal Server Error")
      end)

      job = %Oban.Job{
        args: %{
          "webhook_id" => wid,
          "payload" => %{},
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        }
      }

      assert {:error, "HTTP 500"} = WebhookDelivery.perform(job)

      [row] = WebhookDeliveries.list(delivery_id: delivery_id)
      assert row.status == "retrying"
      assert row.attempt_count == 1
    end

    test "marks the final retry as discarded", %{bypass: bypass, webhook_id: wid} do
      delivery_id = Canary.ID.generate("DLV")

      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        Plug.Conn.resp(conn, 500, "Internal Server Error")
      end)

      job = %Oban.Job{
        attempt: 4,
        max_attempts: 4,
        args: %{
          "webhook_id" => wid,
          "payload" => %{},
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        }
      }

      assert {:error, "HTTP 500"} = WebhookDelivery.perform(job)

      [row] = WebhookDeliveries.list(delivery_id: delivery_id)
      assert row.status == "discarded"
      assert row.reason == "http_500"
      assert row.attempt_count == 1
      assert is_binary(row.discarded_at)
    end

    test "returns error on connection failure", %{bypass: bypass, webhook_id: wid} do
      Bypass.down(bypass)
      delivery_id = Canary.ID.generate("DLV")

      job = %Oban.Job{
        args: %{
          "webhook_id" => wid,
          "payload" => %{},
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        }
      }

      assert {:error, _reason} = WebhookDelivery.perform(job)

      [row] = WebhookDeliveries.list(delivery_id: delivery_id)
      assert row.status == "retrying"
      assert row.attempt_count == 1
    end

    test "discards when webhook not found" do
      delivery_id = Canary.ID.generate("DLV")

      job = %Oban.Job{
        args: %{
          "webhook_id" => "WHK-nonexistent",
          "payload" => %{},
          "event" => "x",
          "delivery_id" => delivery_id
        }
      }

      assert :ok = WebhookDelivery.perform(job)

      [row] = WebhookDeliveries.list(delivery_id: delivery_id)
      assert row.status == "discarded"
      assert row.reason == "webhook_not_found"
    end

    test "skips when webhook inactive", %{bypass: bypass, webhook_id: wid} do
      Repo.get!(Webhook, wid)
      |> Webhook.changeset(%{active: 0})
      |> Repo.update!()

      delivery_id = Canary.ID.generate("DLV")

      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      job = %Oban.Job{
        args: %{
          "webhook_id" => wid,
          "payload" => %{},
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        }
      }

      assert :ok = WebhookDelivery.perform(job)
      refute_received :webhook_delivered

      [row] = WebhookDeliveries.list(delivery_id: delivery_id)
      assert row.status == "discarded"
      assert row.reason == "webhook_inactive"
    end
  end

  describe "enqueue_for_event/2" do
    test "records cooldown suppression in ledger", %{bypass: bypass} do
      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      payload = %{"event" => "error.new_class", "error" => %{"group_hash" => "grp-cooldown"}}
      WebhookDelivery.enqueue_for_event("error.new_class", payload)
      WebhookDelivery.enqueue_for_event("error.new_class", payload)

      assert_receive :webhook_delivered
      refute_received :webhook_delivered

      deliveries = WebhookDeliveries.list(event: "error.new_class")

      assert Enum.any?(
               deliveries,
               &(&1.status == "suppressed" and &1.reason == "cooldown")
             )
    end

    test "delivers when cooldown not active", %{bypass: bypass} do
      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        Plug.Conn.resp(conn, 200, "ok")
      end)

      WebhookDelivery.enqueue_for_event("error.new_class", %{"data" => "test"})
    end

    test "uses payload identity in cooldown key so distinct groups are not collapsed", %{
      bypass: bypass
    } do
      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      WebhookDelivery.enqueue_for_event("error.new_class", %{
        "event" => "error.new_class",
        "error" => %{"group_hash" => "grp-a"}
      })

      WebhookDelivery.enqueue_for_event("error.new_class", %{
        "event" => "error.new_class",
        "error" => %{"group_hash" => "grp-b"}
      })

      assert_receive :webhook_delivered
      assert_receive :webhook_delivered

      deliveries = WebhookDeliveries.list(event: "error.new_class")
      assert Enum.count(deliveries) == 2
      assert Enum.all?(deliveries, &(&1.status == "delivered"))
    end

    test "canonicalizes fallback payload identity before hashing", %{bypass: bypass} do
      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      WebhookDelivery.enqueue_for_event("error.new_class", %{
        "alpha" => 1,
        "nested" => %{"a" => 1, "b" => 2},
        "timestamp" => "2026-04-02T12:00:00Z"
      })

      WebhookDelivery.enqueue_for_event("error.new_class", %{
        "nested" => %{"b" => 2, "a" => 1},
        "timestamp" => "2026-04-02T12:05:00Z",
        "alpha" => 1
      })

      assert_receive :webhook_delivered
      refute_received :webhook_delivered

      deliveries = WebhookDeliveries.list(event: "error.new_class")

      assert Enum.any?(deliveries, &(&1.status == "suppressed" and &1.reason == "cooldown"))
    end

    test "skips webhooks not subscribed to event", %{bypass: bypass} do
      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      WebhookDelivery.enqueue_for_event("unsubscribed.event", %{"data" => "test"})
      refute_received :webhook_delivered
    end
  end

  describe "max_attempts" do
    test "configured to 4" do
      assert WebhookDelivery.__opts__()[:max_attempts] == 4
    end
  end

  describe "backoff/1" do
    test "returns escalating delays" do
      assert WebhookDelivery.backoff(%Oban.Job{attempt: 1}) == 1
      assert WebhookDelivery.backoff(%Oban.Job{attempt: 2}) == 5
      assert WebhookDelivery.backoff(%Oban.Job{attempt: 3}) == 30
      assert WebhookDelivery.backoff(%Oban.Job{attempt: 4}) == 60
    end
  end
end
