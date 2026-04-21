defmodule Canary.Errors.RateLimiter do
  @moduledoc """
  ETS-backed token bucket rate limiter. Per-key limits for error
  ingest and query API. Returns seconds until retry on exhaustion.

  Pure ETS accessors — the `:canary_rate_limits` table is owned and swept by
  `Canary.EtsTables`.
  """

  @table :canary_rate_limits
  @default_limit 100
  @default_window_ms 60_000
  @burst_limit 200
  @burst_window_ms 10_000
  @query_limit 30
  @auth_fail_limit 10

  @spec check(String.t(), atom()) :: :ok | {:error, pos_integer()}
  def check(key, type \\ :ingest) do
    {limit, window} = limits_for(type)
    now = System.monotonic_time(:millisecond)
    bucket_key = {type, key}

    case :ets.lookup(@table, bucket_key) do
      [{_, count, window_start}] when now - window_start < window ->
        if count >= limit do
          retry_after = div(window - (now - window_start), 1_000) + 1
          {:error, retry_after}
        else
          :ets.update_counter(@table, bucket_key, {2, 1})
          :ok
        end

      _ ->
        :ets.insert(@table, {bucket_key, 1, now})
        :ok
    end
  end

  defp limits_for(:ingest), do: {@default_limit, @default_window_ms}
  defp limits_for(:burst), do: {@burst_limit, @burst_window_ms}
  defp limits_for(:query), do: {@query_limit, @default_window_ms}
  defp limits_for(:auth_fail), do: {@auth_fail_limit, @default_window_ms}
end
