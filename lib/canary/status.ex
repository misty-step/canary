defmodule Canary.Status do
  @moduledoc """
  Combined status for agent queries. Merges health check state
  and error counts into a single agent-digestible response.
  """

  alias Canary.Query

  @spec combined(String.t()) :: {:ok, map()} | {:error, :invalid_window}
  def combined(window \\ "1h") do
    with {:ok, error_summary} <- Query.error_summary(window) do
      {:ok, from_snapshot(Query.health_targets(), error_summary, window)}
    end
  end

  @spec from_snapshot(list(), list(), String.t()) :: map()
  def from_snapshot(targets, error_summary, window \\ "1h") do
    targets = Enum.map(targets, &legacy_target/1)
    overall = compute_overall(targets, error_summary)

    %{
      overall: overall,
      summary: Canary.Summary.combined_status(overall, targets, error_summary, window),
      targets: targets,
      error_summary: error_summary
    }
  end

  defp legacy_target(target),
    do: Map.take(target, [:id, :name, :url, :state, :consecutive_failures, :last_checked_at])

  defp compute_overall([], []), do: "empty"

  defp compute_overall([], _errors), do: "warning"

  defp compute_overall(targets, error_summary) do
    has_down = Enum.any?(targets, &(&1.state == "down"))
    has_non_up = Enum.any?(targets, &(&1.state != "up"))
    has_errors = error_summary != []

    cond do
      has_down -> "unhealthy"
      has_non_up -> "degraded"
      has_errors -> "warning"
      true -> "healthy"
    end
  end
end
