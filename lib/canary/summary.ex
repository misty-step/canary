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
          |> Enum.map(& &1.name)
          |> Enum.join(", ")

        parts ++ [", #{degraded} degraded (#{degraded_names})"]
      else
        parts
      end

    parts =
      if down > 0 do
        down_names =
          (by_state["down"] || [])
          |> Enum.map(& &1.name)
          |> Enum.join(", ")

        parts ++ [", #{down} down (#{down_names})"]
      else
        parts
      end

    Enum.join(parts) <> "."
  end

  @spec error_detail(map()) :: String.t()
  def error_detail(%{error_class: error_class, service: service, count: count, first_seen: first_seen, last_seen: last_seen}) do
    "#{error_class} in #{service}. Seen #{count} times since #{first_seen}. Last occurrence: #{last_seen}."
  end
end
