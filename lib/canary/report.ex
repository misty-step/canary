defmodule Canary.Report do
  @moduledoc """
  Agent-first system report composed from health, errors, and recent transitions.
  """

  alias Canary.{Query, Status}

  @default_error_group_limit 25

  @type generate_error :: :invalid_cursor | :invalid_limit | :invalid_window

  @spec generate(keyword()) :: {:ok, map()} | {:error, generate_error()}
  def generate(opts \\ []) do
    window = Keyword.get(opts, :window) || "1h"

    with {:ok, pagination} <- pagination_opts(opts),
         {:ok, slice} <- Query.report_slice(window) do
      targets = Query.health_targets()
      status = Status.from_snapshot(targets, slice.error_summary, window)
      {targets, next_targets_offset} = paginate_targets(targets, pagination)

      {error_groups, next_error_groups_offset} =
        paginate_error_groups(slice.error_groups, pagination)

      truncated = not is_nil(next_targets_offset) or not is_nil(next_error_groups_offset)

      {:ok,
       %{
         status: status.overall,
         summary: status.summary,
         targets: targets,
         error_groups: error_groups,
         incidents: slice.incidents,
         recent_transitions: slice.recent_transitions,
         truncated: truncated,
         cursor: encode_cursor(next_targets_offset, next_error_groups_offset)
       }}
    end
  end

  defp pagination_opts(opts) do
    with {:ok, limit} <- parse_limit(Keyword.get(opts, :limit)),
         {:ok, cursor} <- decode_cursor(Keyword.get(opts, :cursor)) do
      {:ok,
       %{
         limit: limit,
         target_offset: cursor.targets_offset,
         error_group_offset: cursor.error_groups_offset
       }}
    end
  end

  defp parse_limit(nil), do: {:ok, nil}
  defp parse_limit(limit) when is_integer(limit) and limit > 0, do: {:ok, limit}

  defp parse_limit(limit) when is_binary(limit) do
    case Integer.parse(limit) do
      {value, ""} when value > 0 -> {:ok, value}
      _ -> {:error, :invalid_limit}
    end
  end

  defp parse_limit(_), do: {:error, :invalid_limit}

  defp decode_cursor(nil), do: {:ok, %{targets_offset: 0, error_groups_offset: 0}}
  defp decode_cursor(""), do: {:ok, %{targets_offset: 0, error_groups_offset: 0}}

  defp decode_cursor(cursor) when is_binary(cursor) do
    with {:ok, decoded} <- Base.url_decode64(cursor, padding: false),
         {:ok, offsets} <- Jason.decode(decoded),
         {:ok, targets_offset} <- parse_cursor_offset(Map.get(offsets, "targets_offset")),
         {:ok, error_groups_offset} <-
           parse_cursor_offset(Map.get(offsets, "error_groups_offset")) do
      {:ok, %{targets_offset: targets_offset, error_groups_offset: error_groups_offset}}
    else
      _ -> {:error, :invalid_cursor}
    end
  end

  defp decode_cursor(_), do: {:error, :invalid_cursor}

  defp parse_cursor_offset(value) when is_integer(value) and value >= 0, do: {:ok, value}
  defp parse_cursor_offset(nil), do: {:ok, nil}
  defp parse_cursor_offset(_), do: {:error, :invalid_cursor}

  defp paginate_targets(_targets, %{target_offset: nil}), do: {[], nil}

  defp paginate_targets(targets, %{limit: nil, target_offset: offset}),
    do: {Enum.drop(targets, offset), nil}

  defp paginate_targets(targets, %{limit: limit, target_offset: offset}),
    do: paginate(targets, limit, offset)

  defp paginate_error_groups(_error_groups, %{error_group_offset: nil}), do: {[], nil}

  defp paginate_error_groups(error_groups, %{limit: nil, error_group_offset: offset}),
    do: paginate(error_groups, @default_error_group_limit, offset)

  defp paginate_error_groups(error_groups, %{limit: limit, error_group_offset: offset}),
    do: paginate(error_groups, limit, offset)

  defp paginate(items, limit, offset) do
    page = items |> Enum.drop(offset) |> Enum.take(limit)
    next_offset = if length(items) > offset + length(page), do: offset + length(page)
    {page, next_offset}
  end

  defp encode_cursor(nil, nil), do: nil

  defp encode_cursor(targets_offset, error_groups_offset) do
    %{
      targets_offset: targets_offset,
      error_groups_offset: error_groups_offset
    }
    |> Jason.encode!()
    |> Base.url_encode64(padding: false)
  end
end
