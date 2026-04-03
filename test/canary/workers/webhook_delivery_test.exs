defmodule Canary.Workers.WebhookDeliveryTest do
  use Canary.DataCase

  alias Canary.Workers.WebhookDelivery
  alias Canary.Alerter.{CircuitBreaker, Cooldown, Signer}
  alias Canary.Schemas.Webhook
  alias Canary.Schemas.WebhookDelivery, as: WebhookDeliveryRecord
  import Ecto.Query

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

      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        [signature] = Plug.Conn.get_req_header(conn, "x-signature")
        [event] = Plug.Conn.get_req_header(conn, "x-event")

        assert String.starts_with?(signature, "sha256=")
        assert Signer.verify(body, secret, signature)
        assert event == "error.new_class"

        Plug.Conn.resp(conn, 200, "ok")
      end)

      job = %Oban.Job{
        args: %{"webhook_id" => wid, "payload" => payload, "event" => "error.new_class"}
      }

      assert :ok = WebhookDelivery.perform(job)
    end

    test "skips when circuit breaker is open", %{bypass: bypass, webhook_id: wid} do
      for _ <- 1..10, do: CircuitBreaker.record_failure(wid)
      assert CircuitBreaker.open?(wid)

      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      job = %Oban.Job{
        args: %{"webhook_id" => wid, "payload" => %{}, "event" => "error.new_class"}
      }

      assert :ok = WebhookDelivery.perform(job)
      refute_received :webhook_delivered
    end

    test "returns error on non-2xx response", %{bypass: bypass, webhook_id: wid} do
      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        Plug.Conn.resp(conn, 500, "Internal Server Error")
      end)

      job = %Oban.Job{
        args: %{"webhook_id" => wid, "payload" => %{}, "event" => "error.new_class"}
      }

      assert {:error, "HTTP 500"} = WebhookDelivery.perform(job)
    end

    test "reuses the same x-delivery-id across retries from an enqueued delivery", %{
      bypass: bypass,
      webhook_id: wid
    } do
      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        [delivery_id] = Plug.Conn.get_req_header(conn, "x-delivery-id")
        send(test_pid, {:delivery_id, delivery_id})
        Plug.Conn.resp(conn, 500, "retry me")
      end)

      WebhookDelivery.enqueue_for_event("error.new_class", %{"error" => %{"id" => "ERR-test"}})

      delivery =
        from(d in WebhookDeliveryRecord,
          where: d.webhook_id == ^wid and d.event == ^"error.new_class",
          order_by: [desc: d.created_at],
          limit: 1
        )
        |> Repo.one!()

      assert_receive {:delivery_id, first}
      assert first == delivery.id

      retry_job = %Oban.Job{
        args: %{
          "webhook_id" => wid,
          "payload" => %{"error" => %{"id" => "ERR-test"}},
          "event" => "error.new_class",
          "delivery_id" => delivery.id
        },
        attempt: 2,
        max_attempts: 4
      }

      assert {:error, "HTTP 500"} = WebhookDelivery.perform(retry_job)

      assert_receive {:delivery_id, second}
      assert second == first
    end

    test "falls back to a stable job-based delivery id for legacy jobs", %{
      bypass: bypass,
      webhook_id: wid
    } do
      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        [delivery_id] = Plug.Conn.get_req_header(conn, "x-delivery-id")
        send(test_pid, {:delivery_id, delivery_id})
        Plug.Conn.resp(conn, 500, "retry me")
      end)

      job = %Oban.Job{
        id: 123,
        args: %{
          "webhook_id" => wid,
          "payload" => %{"error" => %{"id" => "ERR-test"}},
          "event" => "error.new_class"
        },
        attempt: 1,
        max_attempts: 4
      }

      assert {:error, "HTTP 500"} = WebhookDelivery.perform(job)
      assert {:error, "HTTP 500"} = WebhookDelivery.perform(%{job | attempt: 2})

      assert_receive {:delivery_id, first}
      assert_receive {:delivery_id, second}
      assert first == "DLV-job-123"
      assert second == first
    end

    test "returns error on connection failure", %{bypass: bypass, webhook_id: wid} do
      Bypass.down(bypass)

      job = %Oban.Job{
        args: %{"webhook_id" => wid, "payload" => %{}, "event" => "error.new_class"}
      }

      assert {:error, _reason} = WebhookDelivery.perform(job)
    end

    test "discards when webhook not found" do
      job = %Oban.Job{
        args: %{"webhook_id" => "WHK-nonexistent", "payload" => %{}, "event" => "x"}
      }

      assert :ok = WebhookDelivery.perform(job)
    end

    test "skips when webhook inactive", %{bypass: bypass, webhook_id: wid} do
      Repo.get!(Webhook, wid)
      |> Webhook.changeset(%{active: 0})
      |> Repo.update!()

      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      job = %Oban.Job{
        args: %{"webhook_id" => wid, "payload" => %{}, "event" => "error.new_class"}
      }

      assert :ok = WebhookDelivery.perform(job)
      refute_received :webhook_delivered
    end

    test "records delivered attempts in the ledger", %{bypass: bypass, webhook_id: wid} do
      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        Plug.Conn.resp(conn, 204, "")
      end)

      delivery_id = "DLV-delivered"

      job = %Oban.Job{
        args: %{
          "webhook_id" => wid,
          "payload" => %{"error" => %{"id" => "ERR-test"}},
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        },
        attempt: 1,
        max_attempts: 4
      }

      assert :ok = WebhookDelivery.perform(job)

      delivery = Repo.get!(WebhookDeliveryRecord, delivery_id)
      assert delivery.webhook_id == wid
      assert delivery.event == "error.new_class"
      assert delivery.status == "delivered"
      assert delivery.attempt_count == 1
      assert delivery.last_status_code == 204
      assert delivery.first_attempted_at
      assert delivery.last_attempted_at
      assert delivery.completed_at
      assert is_nil(delivery.suppression_reason)
    end

    test "records discarded deliveries with final attempt count", %{
      bypass: bypass,
      webhook_id: wid
    } do
      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        Plug.Conn.resp(conn, 500, "Internal Server Error")
      end)

      delivery_id = "DLV-discarded"

      base_job = %Oban.Job{
        args: %{
          "webhook_id" => wid,
          "payload" => %{"error" => %{"id" => "ERR-test"}},
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        },
        max_attempts: 4
      }

      for attempt <- 1..4 do
        assert {:error, "HTTP 500"} = WebhookDelivery.perform(%{base_job | attempt: attempt})
      end

      delivery = Repo.get!(WebhookDeliveryRecord, delivery_id)
      assert delivery.status == "discarded"
      assert delivery.attempt_count == 4
      assert delivery.last_status_code == 500
      assert delivery.last_error == "HTTP 500"
      assert delivery.completed_at
    end

    test "records circuit-open suppressions in the ledger", %{bypass: bypass, webhook_id: wid} do
      for _ <- 1..10, do: CircuitBreaker.record_failure(wid)
      assert CircuitBreaker.open?(wid)

      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      delivery_id = "DLV-circuit-open"

      job = %Oban.Job{
        args: %{
          "webhook_id" => wid,
          "payload" => %{"error" => %{"id" => "ERR-test"}},
          "event" => "error.new_class",
          "delivery_id" => delivery_id
        },
        attempt: 1,
        max_attempts: 4
      }

      assert :ok = WebhookDelivery.perform(job)
      refute_received :webhook_delivered

      delivery = Repo.get!(WebhookDeliveryRecord, delivery_id)
      assert delivery.status == "suppressed"
      assert delivery.suppression_reason == "circuit_open"
      assert delivery.attempt_count == 0
      assert delivery.completed_at
    end
  end

  describe "enqueue_for_event/2" do
    test "skips delivery when cooldown active", %{bypass: bypass, webhook_id: wid} do
      Cooldown.mark("#{wid}:error.new_class")

      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      WebhookDelivery.enqueue_for_event("error.new_class", %{"data" => "test"})
      refute_received :webhook_delivered

      [delivery] =
        from(d in WebhookDeliveryRecord,
          where: d.webhook_id == ^wid,
          order_by: [desc: d.created_at]
        )
        |> Repo.all()

      assert delivery.status == "suppressed"
      assert delivery.suppression_reason == "cooldown"
      assert delivery.attempt_count == 0
      assert delivery.completed_at
    end

    test "uses the generated ledger id as the delivery header on enqueue", %{
      bypass: bypass,
      webhook_id: wid
    } do
      test_pid = self()

      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        [delivery_id] = Plug.Conn.get_req_header(conn, "x-delivery-id")
        send(test_pid, {:delivery_id, delivery_id})
        Plug.Conn.resp(conn, 200, "ok")
      end)

      WebhookDelivery.enqueue_for_event("error.new_class", %{"data" => "test"})

      delivery =
        from(d in WebhookDeliveryRecord,
          where: d.webhook_id == ^wid and d.event == ^"error.new_class",
          order_by: [desc: d.created_at],
          limit: 1
        )
        |> Repo.one!()

      assert_receive {:delivery_id, delivery_id}
      assert delivery_id == delivery.id
    end

    test "records a terminal ledger row when enqueue fails", %{bypass: bypass, webhook_id: wid} do
      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        Plug.Conn.resp(conn, 200, "ok")
      end)

      WebhookDelivery.enqueue_for_event("error.new_class", %{"bad" => self()})

      delivery =
        from(d in WebhookDeliveryRecord,
          where: d.webhook_id == ^wid and d.event == ^"error.new_class",
          order_by: [desc: d.created_at],
          limit: 1
        )
        |> Repo.one!()

      assert delivery.status == "discarded"
      assert delivery.completed_at
      assert delivery.last_error =~ "enqueue_failed"
    end

    test "delivers when cooldown not active", %{bypass: bypass} do
      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        Plug.Conn.resp(conn, 200, "ok")
      end)

      WebhookDelivery.enqueue_for_event("error.new_class", %{"data" => "test"})
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
