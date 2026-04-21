defmodule Canary.EtsTables do
  @moduledoc """
  Single owner for Canary's ephemeral ETS tables (cooldown, circuit-breaker,
  dedup-cache, rate-limiter).

  Each table has a matching pure-module wrapper (`Canary.Alerter.Cooldown`,
  `Canary.Alerter.CircuitBreaker`, `Canary.Errors.DedupCache`,
  `Canary.Errors.RateLimiter`) exposing its public API. Those modules read and
  write ETS directly from the caller process — no `GenServer.call` hop.

  This process exists only to own the named tables (so they survive their
  callers) and to sweep expired entries. A single supervision-tree node
  replaces four.
  """

  use GenServer

  @tables [
    %{
      name: :canary_cooldowns,
      sweep_ms: 300_000,
      ttl_ms: 300_000,
      shape: :ts,
      write_concurrency: false
    },
    %{
      name: :canary_circuit_breakers,
      sweep_ms: nil,
      ttl_ms: nil,
      shape: :never_sweep,
      write_concurrency: false
    },
    %{
      name: :canary_dedup_cache,
      sweep_ms: 60_000,
      ttl_ms: 60_000,
      shape: :ts,
      write_concurrency: true
    },
    %{
      name: :canary_rate_limits,
      sweep_ms: 60_000,
      ttl_ms: 120_000,
      shape: :rate_window,
      write_concurrency: true
    }
  ]

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    Enum.each(@tables, &ensure_table/1)
    Enum.each(@tables, &schedule_sweep/1)
    {:ok, %{}}
  end

  @impl true
  def handle_info({:sweep, name}, state) do
    case Enum.find(@tables, &(&1.name == name)) do
      nil ->
        {:noreply, state}

      %{shape: :never_sweep} ->
        {:noreply, state}

      table ->
        sweep(table)
        schedule_sweep(table)
        {:noreply, state}
    end
  end

  defp ensure_table(%{name: name, write_concurrency: write_concurrency}) do
    opts = [:named_table, :public, :set, read_concurrency: true]
    opts = if write_concurrency, do: [{:write_concurrency, true} | opts], else: opts
    :ets.new(name, opts)
  end

  defp schedule_sweep(%{shape: :never_sweep}), do: :ok

  defp schedule_sweep(%{name: name, sweep_ms: sweep_ms}) when is_integer(sweep_ms) do
    Process.send_after(self(), {:sweep, name}, sweep_ms)
    :ok
  end

  # `:ets.select_delete/2` runs entirely inside ERTS — atomic per row and
  # faster than a `foldl` loop that interleaves reads with `:ets.delete/2`.
  # The cutoff is snapshotted once; a row written after the snapshot has a
  # timestamp beyond `cutoff` and survives untouched.
  defp sweep(%{name: name, shape: :ts, ttl_ms: ttl_ms}) do
    cutoff = System.monotonic_time(:millisecond) - ttl_ms
    :ets.select_delete(name, [{{:_, :"$1"}, [{:<, :"$1", cutoff}], [true]}])
  end

  defp sweep(%{name: name, shape: :rate_window, ttl_ms: ttl_ms}) do
    cutoff = System.monotonic_time(:millisecond) - ttl_ms
    :ets.select_delete(name, [{{:_, :_, :"$1"}, [{:<, :"$1", cutoff}], [true]}])
  end
end
