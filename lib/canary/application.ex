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
        Canary.Errors.RateLimiter,
        Canary.Errors.DedupCache,
        Canary.Alerter.CircuitBreaker,
        Canary.Alerter.Cooldown,
        {Registry, keys: :unique, name: Canary.Health.Registry},
        Canary.Health.Supervisor,
        Canary.Health.Manager,
        CanaryWeb.Endpoint
      ]

    result = Supervisor.start_link(children, strategy: :one_for_one, name: Canary.Supervisor)

    # Attach self-monitoring after supervision tree is up
    Canary.ErrorReporter.attach()

    result
  end

  @impl true
  def config_change(changed, _new, removed) do
    CanaryWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
