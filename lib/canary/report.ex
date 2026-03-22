defmodule Canary.Report do
  @moduledoc """
  Agent-first system report composed from health, errors, and recent transitions.
  """

  alias Canary.{Query, Status}

  @spec generate(keyword()) :: {:ok, map()} | {:error, :invalid_window}
  def generate(opts \\ []) do
    window = Keyword.get(opts, :window, "1h")

    with {:ok, error_groups} <- Query.error_groups(window),
         {:ok, recent_transitions} <- Query.recent_transitions(window) do
      targets = Query.health_targets()
      status = Status.combined(window)

      {:ok,
       %{
         status: status.overall,
         summary: status.summary,
         targets: targets,
         error_groups: error_groups,
         recent_transitions: recent_transitions
       }}
    end
  end
end
