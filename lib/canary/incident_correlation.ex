defmodule Canary.IncidentCorrelation do
  @moduledoc false

  alias Canary.Incidents
  alias Canary.Schemas.Incident

  @spec safe_correlate(Incidents.signal_type(), String.t(), String.t()) ::
          {:ok, Incident.t() | nil} | {:error, term()}
  def safe_correlate(signal_type, signal_ref, service) do
    case incident_correlator().correlate(signal_type, signal_ref, service) do
      {:ok, _incident} = ok ->
        ok

      {:error, _reason} = error ->
        error

      other ->
        {:error, {:invalid_return, other}}
    end
  rescue
    error ->
      {:error, {:exception, error.__struct__}}
  catch
    kind, reason ->
      {:error, {kind, reason}}
  end

  defp incident_correlator do
    Application.get_env(:canary, :incident_correlator, Incidents)
  end
end
