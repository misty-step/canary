defmodule Canary.Summary do
  @moduledoc """
  Deterministic template-based summary generation for API responses.
  Pure functions: (query_result) -> string. No side effects.
  """

  @spec error_query(map()) :: String.t()
  def error_query(%{total: total, service: service, window: window, groups: groups}) do
    unique = length(groups)

    top =
      groups
      |> Enum.sort_by(& &1.total_count, :desc)
      |> List.first()

    base = "#{total} errors in #{service} in the last #{window}. #{unique} unique classes."

    case top do
      nil -> base
      t -> "#{base} Most frequent: #{t.error_class} (#{t.total_count} occurrences)."
    end
  end

  @spec health_status(map()) :: String.t()
  def health_status(%{targets: targets}) do
    total = length(targets)
    by_state = Enum.group_by(targets, & &1.state)

    up = length(by_state["up"] || [])
    degraded = length(by_state["degraded"] || [])
    down = length(by_state["down"] || [])

    parts = ["#{total} targets monitored. #{up} up"]

    parts =
      if degraded > 0 do
        degraded_names =
          (by_state["degraded"] || [])
          |> Enum.map_join(", ", & &1.name)

        parts ++ [", #{degraded} degraded (#{degraded_names})"]
      else
        parts
      end

    parts =
      if down > 0 do
        down_names =
          (by_state["down"] || [])
          |> Enum.map_join(", ", & &1.name)

        parts ++ [", #{down} down (#{down_names})"]
      else
        parts
      end

    Enum.join(parts) <> "."
  end

  @spec combined_status(map()) :: String.t()
  def combined_status(%{overall: "empty"}), do: "No services configured."

  def combined_status(%{overall: "healthy", targets: targets}) do
    "All #{length(targets)} targets healthy. No errors in the last hour."
  end

  def combined_status(%{targets: targets, error_summary: error_summary}) do
    by_state = Enum.group_by(targets, & &1.state)
    down = by_state["down"] || []
    degraded = by_state["degraded"] || []
    total_errors = Enum.reduce(error_summary, 0, &(&1.total_count + &2))

    parts = ["#{length(targets)} targets monitored."]

    parts =
      if down != [] do
        names = Enum.map_join(down, ", ", & &1.name)
        parts ++ [" #{length(down)} down (#{names})."]
      else
        parts
      end

    parts =
      if degraded != [] do
        names = Enum.map_join(degraded, ", ", & &1.name)
        parts ++ [" #{length(degraded)} degraded (#{names})."]
      else
        parts
      end

    if total_errors > 0 do
      parts ++ [" #{total_errors} errors across #{length(error_summary)} services in the last hour."]
    else
      parts
    end
    |> Enum.join()
  end

  @spec error_detail(map()) :: String.t()
  def error_detail(%{
        error_class: error_class,
        service: service,
        count: count,
        first_seen: first_seen,
        last_seen: last_seen
      }) do
    "#{error_class} in #{service}. Seen #{count} times since #{first_seen}. Last occurrence: #{last_seen}."
  end
end
