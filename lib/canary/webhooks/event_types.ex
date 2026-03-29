defmodule Canary.Webhooks.EventTypes do
  @moduledoc false

  @events ~w(
    health_check.degraded health_check.down health_check.recovered
    health_check.tls_expiring error.new_class error.regression
    incident.opened incident.updated incident.resolved
  )

  @spec all() :: [String.t()]
  def all, do: @events
  @spec valid?(String.t()) :: boolean()
  def valid?(event), do: event in @events
end
