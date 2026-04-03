defmodule Canary.Webhooks.EventTypes do
  @moduledoc false

  @business ~w(
    health_check.degraded health_check.down health_check.recovered
    health_check.tls_expiring error.new_class error.regression
    incident.opened incident.updated incident.resolved
  )

  @diagnostic ~w(canary.ping)
  @spec all() :: [String.t()]
  def all, do: @business ++ @diagnostic

  @spec business() :: [String.t()]
  def business, do: @business

  @spec diagnostic?(String.t()) :: boolean()
  def diagnostic?(event), do: event in @diagnostic

  @spec valid?(String.t()) :: boolean()
  def valid?(event), do: event in @business or event in @diagnostic
end
