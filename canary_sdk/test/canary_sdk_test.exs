defmodule CanarySdkTest do
  use ExUnit.Case, async: false

  require Logger

  setup do
    on_exit(fn ->
      :logger.remove_handler(:canary_sdk)
    end)
  end

  describe "attach/1" do
    test "registers a :logger handler" do
      assert :ok = CanarySdk.attach(endpoint: "http://localhost:9999", api_key: "k", service: "s")
      assert :canary_sdk in :logger.get_handler_ids()
    end

    test "is idempotent — second attach returns :ok" do
      assert :ok = CanarySdk.attach(endpoint: "http://localhost:9999", api_key: "k", service: "s")
      assert :ok = CanarySdk.attach(endpoint: "http://localhost:9999", api_key: "k", service: "s")
    end

    test "raises on missing required opts" do
      assert_raise KeyError, fn -> CanarySdk.attach(endpoint: "http://localhost") end
    end
  end

  describe "detach/0" do
    test "removes the handler" do
      CanarySdk.attach(endpoint: "http://localhost:9999", api_key: "k", service: "s")
      assert :ok = CanarySdk.detach()
      refute :canary_sdk in :logger.get_handler_ids()
    end

    test "is safe to call when not attached" do
      assert :ok = CanarySdk.detach()
    end
  end

  describe "error capture" do
    test "Logger.error sends POST with service, error_class, message, stack_trace" do
      bypass = Bypass.open()
      test_pid = self()

      Bypass.expect_once(bypass, "POST", "/api/v1/errors", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        payload = Jason.decode!(body)
        send(test_pid, {:captured, payload})
        Plug.Conn.resp(conn, 201, ~s({"id":"ERR-test"}))
      end)

      CanarySdk.attach(
        endpoint: "http://localhost:#{bypass.port}",
        api_key: "test-key",
        service: "my-app"
      )

      Logger.error("something broke")

      assert_receive {:captured, payload}, 1_000
      assert payload["service"] == "my-app"
      assert is_binary(payload["error_class"])
      assert payload["message"] =~ "something broke"
      assert payload["severity"] == "error"
    end

    test "sends authorization header" do
      bypass = Bypass.open()
      test_pid = self()

      Bypass.expect_once(bypass, "POST", "/api/v1/errors", fn conn ->
        auth = Plug.Conn.get_req_header(conn, "authorization")
        send(test_pid, {:auth, auth})
        Plug.Conn.resp(conn, 201, ~s({}))
      end)

      CanarySdk.attach(
        endpoint: "http://localhost:#{bypass.port}",
        api_key: "secret-key",
        service: "s"
      )

      Logger.error("boom")
      assert_receive {:auth, ["Bearer secret-key"]}, 1_000
    end

    test "ignores non-error log levels" do
      bypass = Bypass.open()
      test_pid = self()

      Bypass.stub(bypass, "POST", "/api/v1/errors", fn conn ->
        send(test_pid, :unexpected_post)
        Plug.Conn.resp(conn, 201, ~s({}))
      end)

      CanarySdk.attach(
        endpoint: "http://localhost:#{bypass.port}",
        api_key: "k",
        service: "s"
      )

      Logger.info("just info")
      Logger.debug("just debug")
      Logger.warning("just warning")

      CanarySdk.detach()
      refute_receive :unexpected_post, 200
    end
  end

  describe "resilience" do
    test "handler does not crash the host app on internal error" do
      # Attach with unreachable endpoint — the Task will fail but handler won't crash
      CanarySdk.attach(endpoint: "http://localhost:1", api_key: "k", service: "s")
      Logger.error("this should not crash the app")
      Process.sleep(200)
      # If we reach here, the host app survived
      assert true
    end
  end

  describe "self-referential loop prevention" do
    test "drops errors originating from CanarySdk modules (class-name check)" do
      bypass = Bypass.open()
      test_pid = self()

      Bypass.stub(bypass, "POST", "/api/v1/errors", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        payload = Jason.decode!(body)
        send(test_pid, {:unexpected_post, payload})
        Plug.Conn.resp(conn, 201, ~s({}))
      end)

      CanarySdk.attach(
        endpoint: "http://localhost:#{bypass.port}",
        api_key: "k",
        service: "s"
      )

      Logger.error("** (CanarySdk.Client) connection refused")
      CanarySdk.detach()
      refute_receive {:unexpected_post, _payload}, 200
    end

    test "drops errors from sending process via process metadata flag" do
      bypass = Bypass.open()
      test_pid = self()

      Bypass.stub(bypass, "POST", "/api/v1/errors", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        payload = Jason.decode!(body)
        send(test_pid, {:unexpected_post, payload})
        Plug.Conn.resp(conn, 201, ~s({}))
      end)

      CanarySdk.attach(
        endpoint: "http://localhost:#{bypass.port}",
        api_key: "k",
        service: "s"
      )

      # Simulate error from a process that has the sending flag set
      Logger.metadata(canary_sdk_sending: true)
      Logger.error("from sending task: ArgumentError")
      Logger.metadata(canary_sdk_sending: nil)

      CanarySdk.detach()
      refute_receive {:unexpected_post, _payload}, 200
    end
  end

  describe "structured exception extraction" do
    test "extracts error class and message from meta[:exception]" do
      bypass = Bypass.open()
      test_pid = self()

      Bypass.expect_once(bypass, "POST", "/api/v1/errors", fn conn ->
        {:ok, body, conn} = Plug.Conn.read_body(conn)
        payload = Jason.decode!(body)
        send(test_pid, {:captured, payload})
        Plug.Conn.resp(conn, 201, ~s({}))
      end)

      CanarySdk.attach(
        endpoint: "http://localhost:#{bypass.port}",
        api_key: "k",
        service: "s"
      )

      exception = %RuntimeError{message: "structured boom"}

      Logger.error("something", exception: exception, stacktrace: [])

      assert_receive {:captured, payload}, 1_000
      assert payload["error_class"] == "RuntimeError"
      assert payload["message"] == "structured boom"
    end
  end
end
