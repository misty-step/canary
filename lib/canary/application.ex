defmodule Canary.Application do
  @moduledoc false

  use Application

  require Logger

  @impl true
  def start(_type, _args) do
    children =
      [
        Canary.Repo,
        Canary.ReadRepo,
        {Canary.Release, []},
        CanaryWeb.Telemetry,
        {Phoenix.PubSub, name: Canary.PubSub},
        {DNSCluster, query: Application.get_env(:canary, :dns_cluster_query) || :ignore},
        {Finch, name: Canary.Finch, pools: %{default: [size: 25, count: 2]}},
        {Oban, Application.fetch_env!(:canary, Oban)},
        Canary.EtsTables,
        {Registry, keys: :unique, name: Canary.Health.Registry},
        Canary.Health.Supervisor,
        Canary.Health.Manager,
        Canary.Monitors.Manager,
        CanaryWeb.Endpoint
      ]

    case Supervisor.start_link(children, strategy: :one_for_one, name: Canary.Supervisor) do
      {:ok, pid} ->
        Canary.ErrorReporter.attach()
        {:ok, pid}

      error ->
        error
    end
  end

  @impl true
  def config_change(changed, _new, removed) do
    CanaryWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
