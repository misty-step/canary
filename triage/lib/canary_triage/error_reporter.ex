defmodule CanaryTriage.ErrorReporter do
  @moduledoc """
  Reports uncaught exceptions to Canary via HTTP.
  Attached as a :logger handler.
  """

  require Logger

  @spec attach() :: :ok
  def attach do
    case :logger.add_handler(:canary_triage_reporter, __MODULE__, %{}) do
      :ok -> :ok
      {:error, {:already_exist, _}} -> :ok
      {:error, reason} ->
        Logger.warning("Failed to attach error reporter: #{inspect(reason)}")
        :ok
    end
  end

  def log(%{level: level, msg: msg, meta: meta}, _config)
      when level in [:error, :emergency, :alert, :critical] do
    try do
      {error_class, message, stacktrace} = extract_error(msg, meta)

      unless self_referential?(error_class) do
        endpoint = Application.get_env(:canary_triage, :canary_endpoint)
        api_key = Application.get_env(:canary_triage, :canary_api_key)

        if endpoint && api_key do
          body = %{
            service: "canary-triage",
            error_class: error_class,
            message: String.slice(message, 0, 4_096),
            severity: to_string(level),
            environment: "production",
            stack_trace: stacktrace,
            context: %{
              source: "self_reporter",
              pid: inspect(meta[:pid]),
              module: inspect(meta[:mfa] && elem(meta[:mfa], 0))
            }
          }

          # Fire and forget — don't block the logger
          Task.start(fn ->
            Req.post("#{endpoint}/api/v1/errors",
              json: body,
              headers: [{"authorization", "Bearer #{api_key}"}],
              receive_timeout: 5_000,
              retry: false,
              finch: CanaryTriage.Finch
            )
          end)
        end
      end
    rescue
      _ -> :ok
    end
  end

  def log(_event, _config), do: :ok

  defp extract_error({:string, chars}, meta) do
    message = IO.chardata_to_string(chars)
    extract_from_message(message, meta)
  end

  defp extract_error({:report, report}, _meta) when is_map(report) do
    extract_from_message(to_string(Map.get(report, :message, inspect(report))), %{})
  end

  defp extract_error(msg, _meta), do: extract_from_message(inspect(msg), %{})

  defp extract_from_message(message, meta) do
    error_class =
      case Regex.run(~r/\*\* \((\w+(?:\.\w+)*)\)/, message) do
        [_, class] -> class
        _ -> meta[:error_class] || "OTPError"
      end

    stacktrace =
      case Regex.run(~r/((?:\s+\(.+\) .+:\d+: .+\n?)+)/s, message) do
        [_, st] -> String.trim(st)
        _ -> nil
      end

    {error_class, message, stacktrace}
  end

  defp self_referential?(class) do
    class in ["CanaryTriage.ErrorReporter"]
  end
end
