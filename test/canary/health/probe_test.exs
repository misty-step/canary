defmodule Canary.Health.ProbeTest do
  use ExUnit.Case, async: true

  alias Canary.Health.Probe
  alias Canary.Schemas.Target

  defp build_target(url, opts \\ []) do
    %Target{
      id: "TGT-test",
      url: url,
      name: "test",
      method: Keyword.get(opts, :method, "GET"),
      headers: nil,
      timeout_ms: 5_000,
      expected_status: Keyword.get(opts, :expected_status, "200"),
      body_contains: nil,
      created_at: "2026-01-01T00:00:00Z"
    }
  end

  describe "SSRF redirect protection" do
    test "rejects redirect to loopback (127.0.0.1)" do
      bypass = Bypass.open()
      target = build_target("http://localhost:#{bypass.port}/health")

      Bypass.expect_once(bypass, "GET", "/health", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("location", "http://127.0.0.1/secret")
        |> Plug.Conn.send_resp(302, "")
      end)

      assert {:error, result} = Probe.check(target)
      assert result.result == "redirect_not_followed"
      assert result.status_code == 302
    end

    test "rejects redirect to cloud metadata (169.254.169.254)" do
      bypass = Bypass.open()
      target = build_target("http://localhost:#{bypass.port}/health")

      Bypass.expect_once(bypass, "GET", "/health", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("location", "http://169.254.169.254/latest/meta-data/")
        |> Plug.Conn.send_resp(302, "")
      end)

      assert {:error, result} = Probe.check(target)
      assert result.result == "redirect_not_followed"
      assert result.status_code == 302
    end

    test "treats redirect to valid public URL as redirect_not_followed (redirects disabled)" do
      bypass = Bypass.open()
      target = build_target("http://localhost:#{bypass.port}/health")

      Bypass.expect_once(bypass, "GET", "/health", fn conn ->
        conn
        |> Plug.Conn.put_resp_header("location", "https://example.com/ok")
        |> Plug.Conn.send_resp(302, "")
      end)

      # With redirects disabled, ANY redirect is not followed
      assert {:error, result} = Probe.check(target)
      assert result.result == "redirect_not_followed"
      assert result.status_code == 302
    end
  end

  describe "successful probes" do
    test "returns success for 200 response" do
      bypass = Bypass.open()
      target = build_target("http://localhost:#{bypass.port}/health")

      Bypass.expect_once(bypass, "GET", "/health", fn conn ->
        Plug.Conn.send_resp(conn, 200, "OK")
      end)

      assert {:ok, result} = Probe.check(target)
      assert result.status_code == 200
      assert result.result == "success"
    end
  end
end
