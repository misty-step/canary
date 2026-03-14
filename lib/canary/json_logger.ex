defmodule Canary.JSONLogger do
  @moduledoc "JSON structured log formatter for production."

  def format(level, message, timestamp, metadata) do
    {date, time} = timestamp

    ts =
      NaiveDateTime.from_erl!({date, time})
      |> NaiveDateTime.to_iso8601()

    log =
      %{
        time: ts,
        level: to_string(level),
        msg: to_string(message)
      }
      |> add_metadata(metadata)

    [Jason.encode_to_iodata!(log), "\n"]
  rescue
    _ -> "#{inspect({level, message, timestamp, metadata})}\n"
  end

  defp add_metadata(log, metadata) do
    Enum.reduce(metadata, log, fn
      {key, value}, acc when key in [:request_id, :service, :target, :event] ->
        Map.put(acc, key, to_string(value))

      _, acc ->
        acc
    end)
  end
end
