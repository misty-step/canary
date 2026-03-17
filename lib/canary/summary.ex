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

    [
      "#{total} targets monitored. #{up} up",
      describe_group(by_state["degraded"], "degraded"),
      describe_group(by_state["down"], "down")
    ]
    |> Enum.reject(&is_nil/1)
    |> Enum.join(", ")
    |> Kernel.<>(".")
  end

  @spec combined_status(String.t(), list(), list()) :: String.t()
  def combined_status("empty", _targets, _errors), do: "No services configured."

  def combined_status("healthy", targets, _errors) do
    "All #{length(targets)} targets healthy. No errors in the last hour."
  end

  def combined_status(_overall, targets, error_summary) do
    by_state = Enum.group_by(targets, & &1.state)
    total_errors = Enum.reduce(error_summary, 0, &(&1.total_count + &2))

    [
      "#{length(targets)} targets monitored.",
      describe_group(by_state["down"], "down", " "),
      describe_group(by_state["degraded"], "degraded", " "),
      errors_part(total_errors, error_summary)
    ]
    |> Enum.reject(&is_nil/1)
    |> Enum.join()
  end

  @spec error_class_query(map()) :: String.t()
  def error_class_query(%{total: total, error_class: error_class, window: window, groups: groups}) do
    service_count = groups |> Enum.map(& &1.service) |> Enum.uniq() |> length()
    "#{total} errors matching #{error_class} in the last #{window}. #{length(groups)} groups across #{service_count} services."
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

  # --- Helpers ---

  defp describe_group(nil, _label), do: nil
  defp describe_group([], _label), do: nil

  defp describe_group(targets, label) do
    names = Enum.map_join(targets, ", ", & &1.name)
    "#{length(targets)} #{label} (#{names})"
  end

  defp describe_group(nil, _label, _prefix), do: nil
  defp describe_group([], _label, _prefix), do: nil

  defp describe_group(targets, label, prefix) do
    names = Enum.map_join(targets, ", ", & &1.name)
    "#{prefix}#{length(targets)} #{label} (#{names})."
  end

  defp errors_part(0, _summary), do: nil

  defp errors_part(total, error_summary) do
    svc_count = length(error_summary)
    svc_word = if svc_count == 1, do: "service", else: "services"
    " #{total} errors across #{svc_count} #{svc_word} in the last hour."
  end
end
