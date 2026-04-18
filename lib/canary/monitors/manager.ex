defmodule Canary.Monitors.Manager do
  @moduledoc """
  Boots monitor state and periodically evaluates overdue non-HTTP monitors.
  """

  use GenServer

  alias Canary.Monitors

  @evaluate_interval_ms 30_000

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    unless Application.get_env(:canary, :skip_monitor_manager_boot, false) do
      send(self(), :boot)
    end

    {:ok, %{}}
  end

  @impl true
  def handle_info(:boot, state) do
    Monitors.bootstrap()
    schedule_evaluate()
    {:noreply, state}
  end

  @impl true
  def handle_info(:evaluate, state) do
    Monitors.evaluate_overdue()
    schedule_evaluate()
    {:noreply, state}
  end

  defp schedule_evaluate do
    Process.send_after(self(), :evaluate, @evaluate_interval_ms)
  end
end
