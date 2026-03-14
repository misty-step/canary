defmodule Canary.Health.Supervisor do
  @moduledoc "DynamicSupervisor for health checker GenServers."

  use DynamicSupervisor

  def start_link(init_arg) do
    DynamicSupervisor.start_link(__MODULE__, init_arg, name: __MODULE__)
  end

  @impl true
  def init(_init_arg) do
    DynamicSupervisor.init(strategy: :one_for_one)
  end

  def start_checker(target) do
    DynamicSupervisor.start_child(__MODULE__, {Canary.Health.Checker, target})
  end

  def stop_checker(target_id) do
    case Registry.lookup(Canary.Health.Registry, target_id) do
      [{pid, _}] -> DynamicSupervisor.terminate_child(__MODULE__, pid)
      [] -> :ok
    end
  end
end
