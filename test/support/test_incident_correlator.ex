defmodule Canary.TestIncidentCorrelator do
  @moduledoc false

  def correlate(_signal_type, _signal_ref, _service) do
    case Application.get_env(
           :canary,
           :test_incident_correlator_outcome,
           {:error, {:exception, RuntimeError}}
         ) do
      :raise ->
        raise RuntimeError, "boom"

      :throw ->
        throw(:boom)

      outcome ->
        outcome
    end
  end
end
