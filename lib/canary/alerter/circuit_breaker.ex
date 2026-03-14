defmodule Canary.Alerter.CircuitBreaker do
  @moduledoc """
  Per-subscription circuit breaker. After 10 consecutive delivery
  failures, mark subscription as suspended. Probe every 5 minutes.
  """

  use GenServer

  @table :canary_circuit_breakers
  @failure_threshold 10
  @probe_interval_ms 300_000

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @spec open?(String.t()) :: boolean()
  def open?(webhook_id) do
    case :ets.lookup(@table, webhook_id) do
      [{_, failures, _suspended_at}] when failures >= @failure_threshold -> true
      _ -> false
    end
  end

  @spec should_probe?(String.t()) :: boolean()
  def should_probe?(webhook_id) do
    now = System.monotonic_time(:millisecond)

    case :ets.lookup(@table, webhook_id) do
      [{_, failures, suspended_at}]
      when failures >= @failure_threshold and now - suspended_at >= @probe_interval_ms ->
        true

      _ ->
        false
    end
  end

  @spec record_success(String.t()) :: :ok
  def record_success(webhook_id) do
    :ets.delete(@table, webhook_id)
    :ok
  end

  @spec record_failure(String.t()) :: :ok
  def record_failure(webhook_id) do
    now = System.monotonic_time(:millisecond)

    case :ets.lookup(@table, webhook_id) do
      [{_, failures, _}] ->
        :ets.insert(@table, {webhook_id, failures + 1, now})

      [] ->
        :ets.insert(@table, {webhook_id, 1, now})
    end

    :ok
  end

  @impl true
  def init(_opts) do
    :ets.new(@table, [:named_table, :public, :set, read_concurrency: true])
    {:ok, %{}}
  end
end
