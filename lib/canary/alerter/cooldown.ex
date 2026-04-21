defmodule Canary.Alerter.Cooldown do
  @moduledoc """
  Per-group webhook cooldown. Prevents flood from flapping or
  exception loops. 5 minute default cooldown per group_hash.

  Pure ETS accessors — the `:canary_cooldowns` table is owned and swept by
  `Canary.EtsTables`.
  """

  @table :canary_cooldowns
  @cooldown_ms 300_000

  @spec in_cooldown?(String.t()) :: boolean()
  def in_cooldown?(group_key) do
    now = System.monotonic_time(:millisecond)

    case :ets.lookup(@table, group_key) do
      [{_, ts}] when now - ts < @cooldown_ms -> true
      _ -> false
    end
  end

  @spec mark(String.t()) :: :ok
  def mark(group_key) do
    now = System.monotonic_time(:millisecond)
    :ets.insert(@table, {group_key, now})
    :ok
  end
end
