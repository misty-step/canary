defmodule CanarySdk.Handler do
  @moduledoc false

  @error_levels [:error, :emergency, :alert, :critical]

  def log(%{level: level, msg: msg, meta: meta}, %{config: config})
      when level in @error_levels do
    try do
      {error_class, message, stacktrace} = extract_error(msg, meta)

      unless self_referential?(error_class) do
        body = %{
          service: config.service,
          error_class: error_class,
          message: String.slice(message, 0, 4_096),
          severity: to_string(level),
          environment: config.environment,
          stack_trace: stacktrace,
          context: %{
            source: "canary_sdk",
            pid: inspect(meta[:pid]),
            module: inspect(meta[:mfa] && elem(meta[:mfa], 0))
          }
        }

        Task.start(fn ->
          Req.post("#{config.endpoint}/api/v1/errors",
            json: body,
            headers: [{"authorization", "Bearer #{config.api_key}"}],
            receive_timeout: 5_000,
            retry: false
          )
        end)
      end
    rescue
      _ -> :ok
    end
  end

  def log(_event, _config), do: :ok

  defp extract_error({:string, chars}, _meta) do
    message = IO.chardata_to_string(chars)
    extract_from_message(message)
  end

  defp extract_error({:report, report}, _meta) when is_map(report) do
    extract_from_message(to_string(Map.get(report, :message, inspect(report))))
  end

  defp extract_error(msg, _meta), do: extract_from_message(inspect(msg))

  defp extract_from_message(message) do
    error_class =
      case Regex.run(~r/\*\* \((\w+(?:\.\w+)*)\)/, message) do
        [_, class] -> class
        _ -> "OTPError"
      end

    stacktrace =
      case Regex.run(~r/((?:\s+\(.+\) .+:\d+: .+\n?)+)/s, message) do
        [_, st] -> String.trim(st)
        _ -> nil
      end

    {error_class, message, stacktrace}
  end

  defp self_referential?(class) do
    String.starts_with?(class, "CanarySdk")
  end
end
