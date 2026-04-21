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
    %{name: :canary_cooldowns, sweep_ms: 300_000, ttl_ms: 300_000, shape: :ts},
    %{name: :canary_circuit_breakers, sweep_ms: nil, ttl_ms: nil, shape: :never_sweep},
    %{name: :canary_dedup_cache, sweep_ms: 60_000, ttl_ms: 60_000, shape: :ts},
    %{name: :canary_rate_limits, sweep_ms: 60_000, ttl_ms: 120_000, shape: :rate_window}
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

  defp ensure_table(%{name: name, shape: :rate_window}) do
    :ets.new(name, [
      :named_table,
      :public,
      :set,
      read_concurrency: true,
      write_concurrency: true
    ])
  end

  defp ensure_table(%{name: name, shape: :ts}) do
    opts = [:named_table, :public, :set, read_concurrency: true]

    opts =
      case name do
        :canary_dedup_cache -> [{:write_concurrency, true} | opts]
        _ -> opts
      end

    :ets.new(name, opts)
  end

  defp ensure_table(%{name: name}) do
    :ets.new(name, [:named_table, :public, :set, read_concurrency: true])
  end

  defp schedule_sweep(%{shape: :never_sweep}), do: :ok

  defp schedule_sweep(%{name: name, sweep_ms: sweep_ms}) when is_integer(sweep_ms) do
    Process.send_after(self(), {:sweep, name}, sweep_ms)
    :ok
  end

  defp sweep(%{name: name, shape: :ts, ttl_ms: ttl_ms}) do
    now = System.monotonic_time(:millisecond)

    :ets.foldl(
      fn {key, ts}, acc ->
        if now - ts > ttl_ms, do: :ets.delete(name, key)
        acc
      end,
      nil,
      name
    )
  end

  defp sweep(%{name: name, shape: :rate_window, ttl_ms: ttl_ms}) do
    now = System.monotonic_time(:millisecond)

    :ets.foldl(
      fn {key, _count, window_start}, acc ->
        if now - window_start > ttl_ms, do: :ets.delete(name, key)
        acc
      end,
      nil,
      name
    )
  end
end
