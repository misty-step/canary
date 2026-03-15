defmodule Canary.Workers.WebhookDeliveryTest do
  use Canary.DataCase

  alias Canary.Workers.WebhookDelivery
  alias Canary.Alerter.{CircuitBreaker, Cooldown, Signer}
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

    test "skips when circuit breaker is open", %{webhook_id: wid} do
      for _ <- 1..10, do: CircuitBreaker.record_failure(wid)
      assert CircuitBreaker.open?(wid)

      job = %Oban.Job{
        args: %{"webhook_id" => wid, "payload" => %{}, "event" => "error.new_class"}
      }

      assert :ok = WebhookDelivery.perform(job)
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

    test "skips when webhook inactive", %{webhook_id: wid} do
      Repo.get!(Webhook, wid)
      |> Webhook.changeset(%{active: 0})
      |> Repo.update!()

      job = %Oban.Job{
        args: %{"webhook_id" => wid, "payload" => %{}, "event" => "error.new_class"}
      }

      assert :ok = WebhookDelivery.perform(job)
    end
  end

  describe "enqueue_for_event/2" do
    test "skips delivery when cooldown active", %{bypass: bypass, webhook_id: wid} do
      Cooldown.mark("#{wid}:error.new_class")

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(self(), :webhook_delivered)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      WebhookDelivery.enqueue_for_event("error.new_class", %{"data" => "test"})
      refute_received :webhook_delivered
    end

    test "delivers when cooldown not active", %{bypass: bypass} do
      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        Plug.Conn.resp(conn, 200, "ok")
      end)

      WebhookDelivery.enqueue_for_event("error.new_class", %{"data" => "test"})
    end

    test "skips webhooks not subscribed to event", %{bypass: bypass} do
      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(self(), :webhook_delivered)
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
