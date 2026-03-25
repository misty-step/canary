defmodule Canary.Report do
  @moduledoc """
  Agent-first system report composed from health, errors, and recent transitions.
  """

  alias Canary.{Query, Status}

  @spec generate(keyword()) :: {:ok, map()} | {:error, :invalid_query | :invalid_window}
  def generate(opts \\ []) do
    window = Keyword.get(opts, :window) || "1h"
    search_query = Keyword.get(opts, :q)

    with {:ok, slice} <- Query.report_slice(window),
         {:ok, search_results} <- search_results(search_query, window) do
      targets = Query.health_targets()
      status = Status.from_snapshot(targets, slice.error_summary, window)

      {:ok,
       maybe_put_search_results(
         %{
           status: status.overall,
           summary: status.summary,
           targets: targets,
           error_groups: slice.error_groups,
           incidents: slice.incidents,
           recent_transitions: slice.recent_transitions
         },
         search_results
       )}
    end
  end

  defp search_results(nil, _window), do: {:ok, nil}

  defp search_results(query, window) do
    case Query.search(query, window: window) do
      {:ok, results} -> {:ok, results}
      {:error, :search_failed} -> {:ok, []}
      {:error, reason} -> {:error, reason}
    end
  end

  defp maybe_put_search_results(report, nil), do: report
  defp maybe_put_search_results(report, results), do: Map.put(report, :search_results, results)
end
