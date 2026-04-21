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
  def health_status(%{targets: targets} = snapshot) do
    monitors =
      if Map.has_key?(snapshot, :monitors), do: Map.get(snapshot, :monitors, []), else: nil

    case monitors do
      nil -> summarize_health(targets, "targets")
      monitors -> summarize_health(targets ++ monitors, "health surfaces")
    end
  end

  @spec combined_status(String.t(), list(), list()) :: String.t()
  def combined_status(overall, targets, error_summary) do
    combined_status_legacy(overall, targets, error_summary, "1h")
  end

  @spec combined_status(String.t(), list(), list(), String.t()) :: String.t()
  def combined_status(overall, targets, error_summary, window) when is_binary(window) do
    combined_status_legacy(overall, targets, error_summary, window)
  end

  @spec combined_status(String.t(), list(), list(), list()) :: String.t()
  def combined_status(overall, targets, monitors, error_summary) do
    combined_status_surfaces(overall, targets, monitors, error_summary, "1h")
  end

  @spec combined_status(String.t(), list(), list(), list(), String.t()) :: String.t()
  def combined_status(overall, targets, monitors, error_summary, window) do
    combined_status_surfaces(overall, targets, monitors, error_summary, window)
  end

  @spec error_class_query(map()) :: String.t()
  def error_class_query(%{total: total, error_class: error_class, window: window, groups: groups}) do
    service_count = groups |> Enum.map(& &1.service) |> Enum.uniq() |> length()

    "#{total} errors matching #{error_class} in the last #{window}. #{length(groups)} groups across #{service_count} services."
  end

  @spec error_class_aggregate(map()) :: String.t()
  def error_class_aggregate(%{total: total, window: window, groups: groups}) do
    class_count = length(groups)

    base =
      "#{total} errors across #{class_count} #{pluralize(class_count, "error class", "error classes")} in the last #{window}."

    case Enum.sort_by(groups, & &1.total_count, :desc) do
      [top | _] -> "#{base} Most frequent: #{top.error_class} (#{top.total_count} occurrences)."
      [] -> base
    end
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

  defp errors_part(0, _summary, _window), do: nil

  defp errors_part(total, error_summary, window) do
    svc_count = length(error_summary)
    svc_word = if svc_count == 1, do: "service", else: "services"
    " #{total} errors across #{svc_count} #{svc_word} in the last #{window_label(window)}."
  end

  defp summarize_health(surfaces, label) do
    total = length(surfaces)
    by_state = Enum.group_by(surfaces, & &1.state)
    up = length(by_state["up"] || [])

    [
      "#{total} #{label} monitored. #{up} up",
      describe_group(by_state["degraded"], "degraded"),
      describe_group(by_state["down"], "down")
    ]
    |> Enum.reject(&is_nil/1)
    |> Enum.join(", ")
    |> Kernel.<>(".")
  end

  defp combined_status_legacy("empty", _targets, _errors, _window), do: "No services configured."

  defp combined_status_legacy("healthy", targets, _errors, window) do
    "All #{length(targets)} targets healthy. No errors in the last #{window_label(window)}."
  end

  defp combined_status_legacy(overall, targets, error_summary, window) do
    combined_status_body(overall, targets, error_summary, window, "targets")
  end

  defp combined_status_surfaces("empty", _targets, _monitors, _errors, _window),
    do: "No services configured."

  defp combined_status_surfaces("healthy", targets, monitors, _errors, window) do
    "All #{length(targets ++ monitors)} health surfaces healthy. No errors in the last #{window_label(window)}."
  end

  defp combined_status_surfaces(overall, targets, monitors, error_summary, window) do
    combined_status_body(overall, targets ++ monitors, error_summary, window, "health surfaces")
  end

  defp combined_status_body(_overall, surfaces, error_summary, window, label) do
    by_state = Enum.group_by(surfaces, & &1.state)
    total_errors = Enum.reduce(error_summary, 0, &(&1.total_count + &2))

    [
      "#{length(surfaces)} #{label} monitored.",
      describe_group(by_state["down"], "down", " "),
      describe_group(by_state["degraded"], "degraded", " "),
      errors_part(total_errors, error_summary, window)
    ]
    |> Enum.reject(&is_nil/1)
    |> Enum.join()
  end

  defp pluralize(1, singular, _plural), do: singular
  defp pluralize(_n, _singular, plural), do: plural

  defp window_label("1h"), do: "hour"
  defp window_label("6h"), do: "6 hours"
  defp window_label("24h"), do: "24 hours"
  defp window_label("7d"), do: "7 days"
  defp window_label("30d"), do: "30 days"
  defp window_label(_window), do: "requested window"
end
