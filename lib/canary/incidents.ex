defmodule Canary.Incidents do
  @moduledoc """
  Deterministic incident correlation across health transitions and error groups.
  """

  alias Canary.Incidents.Correlation
  alias Canary.Schemas.Incident

  @type signal_type :: :health_transition | :error_group

  @spec correlate(signal_type(), String.t(), String.t()) ::
          {:ok, Incident.t() | nil} | {:error, term()}
  defdelegate correlate(signal_type, signal_ref, service), to: Correlation
end
