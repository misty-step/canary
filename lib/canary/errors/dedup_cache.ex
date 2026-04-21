defmodule Canary.Errors.DedupCache do
  @moduledoc """
  ETS cache tracking recent group_hashes for webhook deduplication.
  If same group_hash seen within 60s, suppress duplicate webhooks.

  Pure ETS accessors — the `:canary_dedup_cache` table is owned and swept by
  `Canary.EtsTables`.
  """

  @table :canary_dedup_cache
  @window_ms 60_000

  @spec seen_recently?(String.t()) :: boolean()
  def seen_recently?(group_hash) do
    now = System.monotonic_time(:millisecond)

    case :ets.lookup(@table, group_hash) do
      [{_, ts}] when now - ts < @window_ms -> true
      _ -> false
    end
  end

  @spec mark(String.t()) :: :ok
  def mark(group_hash) do
    now = System.monotonic_time(:millisecond)
    :ets.insert(@table, {group_hash, now})
    :ok
  end
end
