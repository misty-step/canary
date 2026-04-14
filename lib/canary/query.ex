defmodule Canary.Query do
  @moduledoc """
  Query API logic. Reads from errors, targets, and incidents.
  All responses include deterministic summary strings.
  """

  defdelegate errors_by_service(service, window, opts \\ []), to: Canary.Query.Errors
  defdelegate errors_by_error_class(error_class, window, opts \\ []), to: Canary.Query.Errors
  defdelegate errors_by_class(window), to: Canary.Query.Errors
  defdelegate error_detail(error_id), to: Canary.Query.Errors
  defdelegate error_groups(window), to: Canary.Query.Errors
  defdelegate error_summary(window), to: Canary.Query.Errors

  defdelegate health_targets(), to: Canary.Query.Health
  defdelegate health_status(), to: Canary.Query.Health
  defdelegate target_checks(target_id, window), to: Canary.Query.Health
  defdelegate recent_transitions(window), to: Canary.Query.Health

  defdelegate active_incidents(opts \\ []), to: Canary.Query.Incidents

  @spec search(String.t(), keyword()) :: {:ok, list(map())} | {:error, atom()}
  def search(query, opts \\ []) do
    case Keyword.get(opts, :window) do
      nil ->
        Canary.Query.Search.search(query, opts)

      window ->
        with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
          Canary.Query.Search.search(query, Keyword.put(opts, :cutoff, cutoff))
        end
    end
  end

  @spec report_slice(String.t()) :: {:ok, map()} | {:error, :invalid_window}
  def report_slice(window) do
    with {:ok, cutoff} <- Canary.Query.Window.to_cutoff(window) do
      {:ok,
       %{
         error_groups: Canary.Query.Errors.error_groups_since(cutoff),
         error_summary: Canary.Query.Errors.error_summary_since(cutoff),
         incidents: active_incidents(),
         recent_transitions: Canary.Query.Health.recent_transitions_since(cutoff)
       }}
    end
  end
end
