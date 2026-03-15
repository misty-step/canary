defmodule CanaryTriage.DispatchTest do
  use ExUnit.Case, async: true

  alias CanaryTriage.Dispatch

  @secret "test-secret"

  defp sign(body) do
    "sha256=" <> (:crypto.mac(:hmac, :sha256, @secret, body) |> Base.encode16(case: :lower))
  end

  defp stub_github(handler), do: Req.Test.stub(CanaryTriage.GitHub, handler)

  setup do
    Application.put_env(:canary_triage, :service_repos, %{"canary-triage" => "misty-step/canary"})
    on_exit(fn -> Application.delete_env(:canary_triage, :service_repos) end)
    :ok
  end

  defp health_payload(event, state, prev \\ "healthy") do
    %{
      "event" => event,
      "state" => state,
      "previous_state" => prev,
      "timestamp" => "2026-03-15T10:00:00Z",
      "consecutive_failures" => 3,
      "last_success_at" => "2026-03-15T09:55:00Z",
      "target" => %{"name" => "canary-triage", "url" => "https://canary-triage.fly.dev/healthz"},
      "last_check" => %{"result" => "timeout", "status_code" => 0, "latency_ms" => 5000}
    }
  end

  describe "degraded event" do
    test "creates new issue when no existing health issue" do
      payload = health_payload("health_check.degraded", "degraded")
      body = Jason.encode!(payload)

      stub_github(fn conn ->
        case conn.method do
          "GET" -> Req.Test.json(conn, [])
          "POST" -> conn |> Plug.Conn.put_status(201) |> Req.Test.json(%{"number" => 1, "html_url" => "https://github.com/misty-step/canary/issues/1"})
        end
      end)

      assert {:ok, %{"number" => 1}} = Dispatch.handle(body, payload, sign(body))
    end

    test "comments on existing issue instead of creating new one" do
      payload = health_payload("health_check.degraded", "degraded")
      body = Jason.encode!(payload)

      stub_github(fn conn ->
        case conn.method do
          "GET" ->
            Req.Test.json(conn, [
              %{"number" => 42, "title" => "Health Check Degraded: canary-triage", "html_url" => "https://github.com/misty-step/canary/issues/42"}
            ])

          "POST" ->
            conn |> Plug.Conn.put_status(201) |> Req.Test.json(%{"id" => 99})
        end
      end)

      assert {:ok, %{"number" => 42}} = Dispatch.handle(body, payload, sign(body))
    end
  end

  describe "recovered event" do
    test "closes existing health issue with comment" do
      payload = health_payload("health_check.recovered", "healthy", "degraded")
      body = Jason.encode!(payload)

      stub_github(fn conn ->
        case conn.method do
          "GET" ->
            Req.Test.json(conn, [
              %{"number" => 42, "title" => "Health Check Degraded: canary-triage", "html_url" => "https://github.com/misty-step/canary/issues/42"}
            ])

          "POST" ->
            conn |> Plug.Conn.put_status(201) |> Req.Test.json(%{"id" => 99})

          "PATCH" ->
            Req.Test.json(conn, %{"number" => 42, "state" => "closed"})
        end
      end)

      assert {:ok, %{"number" => 42}} = Dispatch.handle(body, payload, sign(body))
    end

    test "no-ops when no matching open issue" do
      payload = health_payload("health_check.recovered", "healthy", "degraded")
      body = Jason.encode!(payload)

      stub_github(fn conn ->
        "GET" = conn.method
        Req.Test.json(conn, [])
      end)

      assert {:ok, :noop} = Dispatch.handle(body, payload, sign(body))
    end
  end

  describe "signature verification" do
    test "rejects invalid signature" do
      payload = health_payload("health_check.degraded", "degraded")
      body = Jason.encode!(payload)

      assert {:error, :invalid_signature} = Dispatch.handle(body, payload, "sha256=bad")
    end
  end
end
