defmodule Canary.AnnotationsWebhookTest do
  use Canary.DataCase

  import Canary.Fixtures

  alias Canary.Alerter.Signer
  alias Canary.Annotations
  alias Canary.Schemas.Webhook
  alias Canary.WebhookDeliveries

  setup do
    bypass = Bypass.open()
    clean_status_tables()

    webhook_id = Canary.ID.webhook_id()
    secret = "test-webhook-secret"
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    %Webhook{id: webhook_id}
    |> Webhook.changeset(%{
      url: "http://localhost:#{bypass.port}/hook",
      events: Jason.encode!(["annotation.added"]),
      secret: secret,
      created_at: now
    })
    |> Canary.Repo.insert!()

    :ets.delete_all_objects(:canary_circuit_breakers)
    :ets.delete_all_objects(:canary_cooldowns)

    on_exit(fn ->
      :ets.delete_all_objects(:canary_circuit_breakers)
      :ets.delete_all_objects(:canary_cooldowns)
    end)

    %{bypass: bypass, webhook_id: webhook_id, secret: secret}
  end

  describe "Annotations.create/1 webhook dispatch" do
    test "fires annotation.added webhook with signed payload and delivery id", %{
      bypass: bypass,
      webhook_id: webhook_id,
      secret: secret
    } do
      test_pid = self()
      group = create_error_group("svc", "RuntimeError", 1)

      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        [signature] = Plug.Conn.get_req_header(conn, "x-signature")
        [event] = Plug.Conn.get_req_header(conn, "x-event")
        [delivery_id] = Plug.Conn.get_req_header(conn, "x-delivery-id")

        assert event == "annotation.added"
        assert Signer.verify(body, secret, signature)
        send(test_pid, {:webhook, delivery_id, Jason.decode!(body)})

        Plug.Conn.resp(conn, 200, "ok")
      end)

      {:ok, annotation} =
        Annotations.create(%{
          "subject_type" => "error_group",
          "subject_id" => group.group_hash,
          "agent" => "fix-bot",
          "action" => "linked",
          "metadata" => %{"pr" => "https://example.com/pr/1"}
        })

      assert_receive {:webhook, delivery_id, payload}, 500
      assert String.starts_with?(delivery_id, "DLV-")
      assert payload["event"] == "annotation.added"
      assert payload["annotation"]["id"] == annotation.id
      assert payload["annotation"]["subject_type"] == "error_group"
      assert payload["annotation"]["subject_id"] == group.group_hash
      assert payload["annotation"]["action"] == "linked"
      assert payload["annotation"]["metadata"] == %{"pr" => "https://example.com/pr/1"}

      [row] = WebhookDeliveries.list(webhook_id: webhook_id, event: "annotation.added")
      assert row.delivery_id == delivery_id
      assert row.status == "delivered"
    end

    test "fires webhook for target subjects too", %{bypass: bypass} do
      test_pid = self()
      create_target_with_state("api", "up")

      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        send(test_pid, {:webhook, Jason.decode!(body)})
        Plug.Conn.resp(conn, 200, "ok")
      end)

      {:ok, _} =
        Annotations.create(%{
          "subject_type" => "target",
          "subject_id" => "TGT-api",
          "agent" => "bot",
          "action" => "paged"
        })

      assert_receive {:webhook, payload}, 500
      assert payload["annotation"]["subject_type"] == "target"
      assert payload["annotation"]["subject_id"] == "TGT-api"
    end

    test "does not fire webhook when subject does not exist", %{bypass: bypass} do
      test_pid = self()

      Bypass.stub(bypass, "POST", "/hook", fn conn ->
        send(test_pid, :unexpected_webhook)
        Plug.Conn.resp(conn, 200, "ok")
      end)

      assert {:error, :not_found} =
               Annotations.create(%{
                 "subject_type" => "target",
                 "subject_id" => "TGT-nope",
                 "agent" => "bot",
                 "action" => "ack"
               })

      refute_received :unexpected_webhook
    end

    test "legacy create_for_incident still fires the webhook", %{bypass: bypass} do
      test_pid = self()
      incident = create_incident("svc")

      Bypass.expect_once(bypass, "POST", "/hook", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        send(test_pid, {:webhook, Jason.decode!(body)})
        Plug.Conn.resp(conn, 200, "ok")
      end)

      {:ok, _} =
        Annotations.create_for_incident(incident.id, %{
          "agent" => "bot",
          "action" => "acknowledged"
        })

      assert_receive {:webhook, payload}, 500
      assert payload["annotation"]["subject_type"] == "incident"
      assert payload["annotation"]["subject_id"] == incident.id
    end
  end
end
