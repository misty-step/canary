defmodule CanarySdkTest do
  use ExUnit.Case, async: false

  require Logger

  setup do
    on_exit(fn ->
      :logger.remove_handler(:canary_sdk)
    end)
  end

  defp capture_request_fun(test_pid) do
    fn url, opts ->
      payload = opts |> Keyword.fetch!(:json) |> Jason.encode!() |> Jason.decode!()
      send(test_pid, {:request, url, opts})
      send(test_pid, {:captured, payload})
      {:ok, %{status: 201}}
    end
  end

  defp assert_captured_payload(matches?, timeout \\ 1_000) do
    deadline = System.monotonic_time(:millisecond) + timeout
    do_assert_captured_payload(matches?, deadline)
  end

  defp do_assert_captured_payload(matches?, deadline) do
    remaining = max(deadline - System.monotonic_time(:millisecond), 0)

    receive do
      {:captured, payload} ->
        if matches?.(payload) do
          payload
        else
          do_assert_captured_payload(matches?, deadline)
        end
    after
      remaining ->
        flunk("expected matching Canary SDK payload")
    end
  end

  defp handler_config(overrides \\ %{}) do
    Map.merge(
      %{
        endpoint: "http://canary.test",
        api_key: "test-key",
        service: "sdk-service",
        environment: "test",
        request_fun: capture_request_fun(self())
      },
      overrides
    )
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
      test_pid = self()

      CanarySdk.attach(
        endpoint: "http://canary.test",
        api_key: "test-key",
        service: "my-app",
        request_fun: capture_request_fun(test_pid)
      )

      Logger.error("something broke")

      payload =
        assert_captured_payload(fn payload ->
          String.contains?(payload["message"], "something broke")
        end)

      assert payload["service"] == "my-app"
      assert is_binary(payload["error_class"])
      assert payload["severity"] == "error"
      CanarySdk.detach()
    end

    test "sends authorization header" do
      test_pid = self()

      CanarySdk.attach(
        endpoint: "http://canary.test",
        api_key: "secret-key",
        service: "s",
        request_fun: capture_request_fun(test_pid)
      )

      Logger.error("boom")

      assert_receive {:request, "http://canary.test/api/v1/errors", opts}, 1_000
      assert Keyword.fetch!(opts, :headers) == [{"authorization", "Bearer secret-key"}]
      CanarySdk.detach()
    end

    test "ignores non-error log levels" do
      test_pid = self()

      CanarySdk.attach(
        endpoint: "http://canary.test",
        api_key: "k",
        service: "s",
        request_fun: fn _url, _opts ->
          send(test_pid, :unexpected_post)
          {:ok, %{status: 201}}
        end
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
      test_pid = self()

      CanarySdk.attach(
        endpoint: "http://canary.test",
        api_key: "k",
        service: "s",
        request_fun: fn _url, opts ->
          send(test_pid, {:unexpected_post, Keyword.fetch!(opts, :json)})
          {:ok, %{status: 201}}
        end
      )

      Logger.error("** (CanarySdk.Client) connection refused")
      CanarySdk.detach()
      refute_receive {:unexpected_post, _payload}, 200
    end

    test "drops errors from sending process via process metadata flag" do
      test_pid = self()

      CanarySdk.attach(
        endpoint: "http://canary.test",
        api_key: "k",
        service: "s",
        request_fun: fn _url, opts ->
          send(test_pid, {:unexpected_post, Keyword.fetch!(opts, :json)})
          {:ok, %{status: 201}}
        end
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
      test_pid = self()

      CanarySdk.attach(
        endpoint: "http://canary.test",
        api_key: "k",
        service: "s",
        request_fun: capture_request_fun(test_pid)
      )

      exception = %RuntimeError{message: "structured boom"}

      Logger.error("something", exception: exception, stacktrace: [])

      payload =
        assert_captured_payload(fn payload ->
          payload["message"] == "structured boom"
        end)

      assert payload["error_class"] == "RuntimeError"
      CanarySdk.detach()
    end

    test "formats exception stacktraces and respects custom environments" do
      test_pid = self()

      CanarySdk.attach(
        endpoint: "http://canary.test",
        api_key: "k",
        service: "s",
        environment: "staging",
        request_fun: capture_request_fun(test_pid)
      )

      stacktrace = [{CanarySdkTest, :sample, 0, [file: ~c"test/canary_sdk_test.exs", line: 222]}]
      exception = %ArgumentError{message: "invalid payload"}

      Logger.error("ignored", exception: exception, stacktrace: stacktrace)

      payload =
        assert_captured_payload(fn payload ->
          payload["message"] == "invalid payload"
        end)

      assert payload["environment"] == "staging"
      assert payload["error_class"] == "ArgumentError"
      assert payload["stack_trace"] =~ "test/canary_sdk_test.exs:222"
      CanarySdk.detach()
    end
  end

  describe "handler payload shaping" do
    test "extracts report messages and stacktraces" do
      event = %{
        level: :error,
        msg:
          {:report,
           %{
             message:
               "** (ArgumentError) bad arg\n    (canary_sdk 0.1.0) lib/canary_sdk.ex:12: CanarySdk.attach/1\n"
           }},
        meta: %{pid: self(), mfa: {CanarySdk, :attach, 1}}
      }

      CanarySdk.Handler.log(event, %{config: handler_config()})

      payload =
        assert_captured_payload(fn payload ->
          payload["error_class"] == "ArgumentError"
        end)

      assert payload["error_class"] == "ArgumentError"
      assert payload["stack_trace"] =~ "lib/canary_sdk.ex:12"
      assert payload["context"]["module"] == "CanarySdk"
    end

    test "falls back to OTPError for generic terms" do
      event = %{
        level: :error,
        msg: %{kind: :unexpected},
        meta: %{pid: self(), mfa: {CanarySdkTest, :sample, 0}}
      }

      CanarySdk.Handler.log(event, %{config: handler_config()})

      payload =
        assert_captured_payload(fn payload ->
          String.contains?(payload["message"], "kind: :unexpected")
        end)

      assert payload["error_class"] == "OTPError"
      assert payload["context"]["module"] == "CanarySdkTest"
    end

    test "truncates long messages and preserves context metadata" do
      long_message = String.duplicate("a", 5_000)

      event = %{
        level: :error,
        msg: {:string, long_message},
        meta: %{pid: self(), mfa: {CanarySdkTest, :sample, 0}}
      }

      CanarySdk.Handler.log(event, %{config: handler_config()})

      payload =
        assert_captured_payload(fn payload ->
          payload["message"] == String.slice(long_message, 0, 4_096)
        end)

      assert String.length(payload["message"]) == 4_096
      assert payload["context"]["source"] == "canary_sdk"
      assert payload["context"]["pid"] == inspect(self())
    end
  end
end
