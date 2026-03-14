defmodule Canary.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      # Data layer
      Canary.Repo,
      Canary.ReadRepo,

      # Telemetry
      CanaryWeb.Telemetry,

      # PubSub
      {Phoenix.PubSub, name: Canary.PubSub},

      # DNS cluster
      {DNSCluster, query: Application.get_env(:canary, :dns_cluster_query) || :ignore},

      # Shared HTTP connection pool
      {Finch, name: Canary.Finch, pools: %{default: [size: 25, count: 2]}},

      # Background jobs
      {Oban, Application.fetch_env!(:canary, Oban)},

      # ETS-backed services
      Canary.Errors.RateLimiter,
      Canary.Errors.DedupCache,
      Canary.Alerter.CircuitBreaker,
      Canary.Alerter.Cooldown,

      # Health checker registry + supervisor + manager
      {Registry, keys: :unique, name: Canary.Health.Registry},
      Canary.Health.Supervisor,
      Canary.Health.Manager,

      # HTTP endpoint (last)
      CanaryWeb.Endpoint
    ]

    opts = [strategy: :one_for_one, name: Canary.Supervisor]

    with {:ok, pid} <- Supervisor.start_link(children, opts) do
      run_migrations()
      run_seeds()
      {:ok, pid}
    end
  end

  @impl true
  def config_change(changed, _new, removed) do
    CanaryWeb.Endpoint.config_change(changed, removed)
    :ok
  end

  defp run_migrations do
    if Application.get_env(:canary, :run_migrations, true) do
      Ecto.Migrator.run(Canary.Repo, :up, all: true)
    end
  rescue
    e -> require Logger; Logger.warning("Migration skipped: #{inspect(e)}")
  end

  defp run_seeds do
    case Canary.Repo.query("SELECT seed_name FROM seed_runs WHERE seed_name = 'initial_config_v1'") do
      {:ok, %{rows: []}} ->
        Canary.Seeds.run()

      {:ok, _} ->
        :ok

      {:error, _} ->
        :ok
    end
  rescue
    _ -> :ok
  end
end
