defmodule Canary.ErrorReporter do
  @moduledoc """
  Reports uncaught exceptions to Canary's own error ingestion.
  Attached as a :logger handler — captures OTP crash reports
  and Logger.error calls with exception metadata.

  Canary monitoring Canary: the error goes directly to the
  ingest pipeline (no HTTP round-trip to self).
  """

  require Logger

  @spec attach() :: :ok
  def attach do
    case :logger.add_handler(:canary_self_reporter, __MODULE__, %{}) do
      :ok -> :ok
      {:error, {:already_exist, _}} -> :ok
    end
  end

  # :logger handler callback
  def log(%{level: level, msg: msg, meta: meta}, _config)
      when level in [:error, :emergency, :alert, :critical] do
    try do
      {error_class, message, stacktrace} = extract_error(msg, meta)

      # Skip if this looks like our own reporting (prevent loops)
      unless self_referential?(error_class) do
        attrs = %{
          "service" => "canary",
          "error_class" => error_class,
          "message" => String.slice(message, 0, 4_096),
          "severity" => to_string(level),
          "environment" => environment(),
          "stack_trace" => stacktrace,
          "context" => %{
            "source" => "self_reporter",
            "pid" => inspect(meta[:pid]),
            "module" => inspect(meta[:mfa] && elem(meta[:mfa], 0))
          }
        }

        # Direct ingest — no HTTP. We ARE the server.
        Canary.Errors.Ingest.ingest(attrs)
      end
    rescue
      # Never crash from error reporting
      _ -> :ok
    end
  end

  def log(_event, _config), do: :ok

  defp extract_error({:string, chars}, meta) do
    message = IO.chardata_to_string(chars)
    extract_from_message(message, meta)
  end

  defp extract_error({:report, report}, meta) when is_map(report) do
    message = Map.get(report, :message, inspect(report))
    extract_from_message(to_string(message), meta)
  end

  defp extract_error({:report, report}, meta) do
    extract_from_message(inspect(report), meta)
  end

  defp extract_error(msg, meta) do
    extract_from_message(inspect(msg), meta)
  end

  defp extract_from_message(message, meta) do
    # Try to extract exception class from the message
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
    class in ["Canary.ErrorReporter", "Canary.Errors.Ingest"]
  end

  defp environment do
    if Application.get_env(:canary, :env) == :prod, do: "production", else: "development"
  end
end
