defmodule Canary.Errors.DedupCache do
  @moduledoc """
  ETS cache tracking recent group_hashes for webhook deduplication.
  If same group_hash seen within 60s, suppress duplicate webhooks.
  """

  use GenServer

  @table :canary_dedup_cache
  @window_ms 60_000

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

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

  @impl true
  def init(_opts) do
    :ets.new(@table, [:named_table, :public, :set, read_concurrency: true, write_concurrency: true])
    schedule_cleanup()
    {:ok, %{}}
  end

  @impl true
  def handle_info(:cleanup, state) do
    now = System.monotonic_time(:millisecond)

    :ets.foldl(
      fn {key, ts}, acc ->
        if now - ts > @window_ms, do: :ets.delete(@table, key)
        acc
      end,
      nil,
      @table
    )

    schedule_cleanup()
    {:noreply, state}
  end

  defp schedule_cleanup, do: Process.send_after(self(), :cleanup, @window_ms)
end
