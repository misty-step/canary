defmodule Canary.Webhooks.EventTypes do
  @moduledoc false

  @timeline ~w(
    health_check.degraded health_check.down health_check.recovered
    health_check.tls_expiring error.new_class error.regression
    incident.opened incident.updated incident.resolved
  )

  @diagnostic ~w(canary.ping)
  @all @timeline ++ @diagnostic

  @spec valid?(String.t()) :: boolean()
  def valid?(event), do: event in @all

  @spec timeline() :: [String.t()]
  def timeline, do: @timeline
end
