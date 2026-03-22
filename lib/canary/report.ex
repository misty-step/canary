defmodule Canary.Report do
  @moduledoc """
  Agent-first system report composed from health, errors, and recent transitions.
  """

  alias Canary.{Query, Status}

  @spec generate(keyword()) :: {:ok, map()} | {:error, :invalid_window}
  def generate(opts \\ []) do
    window = Keyword.get(opts, :window) || "1h"

    with {:ok, slice} <- Query.report_slice(window) do
      targets = Query.health_targets()
      status = Status.from_snapshot(targets, slice.error_summary, window)

      {:ok,
       %{
         status: status.overall,
         summary: status.summary,
         targets: targets,
         error_groups: slice.error_groups,
         incidents: slice.incidents,
         recent_transitions: slice.recent_transitions
       }}
    end
  end
end
