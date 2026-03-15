defmodule CanaryTriage.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      CanaryTriageWeb.Telemetry,
      {DNSCluster, query: Application.get_env(:canary_triage, :dns_cluster_query) || :ignore},
      {Phoenix.PubSub, name: CanaryTriage.PubSub},
      {Finch, name: CanaryTriage.Finch, pools: %{default: [size: 10, count: 1]}},
      CanaryTriageWeb.Endpoint
    ]

    opts = [strategy: :one_for_one, name: CanaryTriage.Supervisor]

    case Supervisor.start_link(children, opts) do
      {:ok, pid} ->
        CanaryTriage.ErrorReporter.attach()
        {:ok, pid}

      error ->
        error
    end
  end

  @impl true
  def config_change(changed, _new, removed) do
    CanaryTriageWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
