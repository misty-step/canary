defmodule Canary.Alerter.Cooldown do
  @moduledoc """
  Per-group webhook cooldown. Prevents flood from flapping or
  exception loops. 5 minute default cooldown per group_hash.
  """

  use GenServer

  @table :canary_cooldowns
  @cooldown_ms 300_000

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

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

  @impl true
  def init(_opts) do
    :ets.new(@table, [:named_table, :public, :set, read_concurrency: true])
    schedule_cleanup()
    {:ok, %{}}
  end

  @impl true
  def handle_info(:cleanup, state) do
    now = System.monotonic_time(:millisecond)

    :ets.foldl(
      fn {key, ts}, acc ->
        if now - ts > @cooldown_ms, do: :ets.delete(@table, key)
        acc
      end,
      nil,
      @table
    )

    schedule_cleanup()
    {:noreply, state}
  end

  defp schedule_cleanup, do: Process.send_after(self(), :cleanup, @cooldown_ms)
end
