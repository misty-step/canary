defmodule Canary.Status do
  @moduledoc """
  Combined status for agent queries. Merges health check state
  and error counts into a single agent-digestible response.
  """

  alias Canary.Schemas.{ErrorGroup, Target, TargetState}
  import Ecto.Query

  @window_seconds 3_600

  @spec combined() :: map()
  def combined do
    targets = fetch_targets()
    error_summary = fetch_error_summary()
    overall = compute_overall(targets, error_summary)

    result = %{overall: overall, targets: targets, error_summary: error_summary}
    summary = Canary.Summary.combined_status(result)

    Map.put(result, :summary, summary)
  end

  defp fetch_targets do
    from(t in Target,
      left_join: s in TargetState,
      on: t.id == s.target_id,
      order_by: t.name,
      select: {t, s}
    )
    |> Canary.Repos.read_repo().all()
    |> Enum.map(fn {target, state} ->
      %{
        id: target.id,
        name: target.name,
        url: target.url,
        state: (state && state.state) || "unknown",
        consecutive_failures: (state && state.consecutive_failures) || 0,
        last_checked_at: state && state.last_checked_at
      }
    end)
  end

  defp fetch_error_summary do
    cutoff =
      DateTime.utc_now()
      |> DateTime.add(-@window_seconds, :second)
      |> DateTime.to_iso8601()

    from(g in ErrorGroup,
      where: g.last_seen_at >= ^cutoff and g.status == "active",
      group_by: g.service,
      select: %{
        service: g.service,
        total_count: sum(g.total_count),
        unique_classes: count(g.group_hash)
      },
      order_by: [desc: sum(g.total_count)]
    )
    |> Canary.Repos.read_repo().all()
  end

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
