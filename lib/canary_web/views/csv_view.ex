defmodule CanaryWeb.CsvView do
  @moduledoc false

  @headers [
    "section",
    "position",
    "id",
    "name",
    "service",
    "error_class",
    "url",
    "state",
    "count",
    "first_seen",
    "last_seen",
    "severity",
    "status",
    "consecutive_failures",
    "last_checked_at",
    "cursor",
    "truncated"
  ]

  def report(data) do
    rows =
      [@headers]
      |> Kernel.++(target_rows(data))
      |> Kernel.++(error_group_rows(data))

    Enum.map_join(rows, "\n", &encode_row/1)
  end

  defp target_rows(data) do
    Enum.with_index(data.targets, 1)
    |> Enum.map(fn {target, position} ->
      [
        "targets",
        position,
        target.id,
        target.name,
        nil,
        nil,
        target.url,
        target.state,
        nil,
        nil,
        nil,
        nil,
        nil,
        target.consecutive_failures,
        target.last_checked_at,
        data.cursor,
        data.truncated
      ]
    end)
  end

  defp error_group_rows(data) do
    Enum.with_index(data.error_groups, 1)
    |> Enum.map(fn {group, position} ->
      [
        "error_groups",
        position,
        nil,
        nil,
        group.service,
        group.error_class,
        nil,
        group.status,
        group.count,
        group.first_seen,
        group.last_seen,
        group.severity,
        group.status,
        nil,
        nil,
        data.cursor,
        data.truncated
      ]
    end)
  end

  defp encode_row(values) do
    values
    |> Enum.map(&encode_value/1)
    |> Enum.join(",")
  end

  defp encode_value(nil), do: ""
  defp encode_value(true), do: "true"
  defp encode_value(false), do: "false"
  defp encode_value(value) when is_integer(value), do: Integer.to_string(value)

  defp encode_value(value) do
    string = to_string(value)

    if String.contains?(string, [",", "\"", "\n", "\r"]) do
      "\"" <> String.replace(string, "\"", "\"\"") <> "\""
    else
      string
    end
  end
end
