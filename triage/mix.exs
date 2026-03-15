defmodule CanaryTriage.MixProject do
  use Mix.Project

  def project do
    [
      app: :canary_triage,
      version: "0.1.0",
      elixir: "~> 1.17",
      elixirc_paths: elixirc_paths(Mix.env()),
      start_permanent: Mix.env() == :prod,
      aliases: aliases(),
      deps: deps()
    ]
  end

  def application do
    [
      mod: {CanaryTriage.Application, []},
      extra_applications: [:logger, :runtime_tools]
    ]
  end

  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_), do: ["lib"]

  defp deps do
    [
      {:phoenix, "~> 1.8"},
      {:req, "~> 0.5"},
      {:finch, "~> 0.19"},
      {:jason, "~> 1.4"},
      {:bandit, "~> 1.5"},
      {:telemetry_metrics, "~> 1.0"},
      {:telemetry_poller, "~> 1.0"},
      {:dns_cluster, "~> 0.2.0"}
    ]
  end

  defp aliases do
    [setup: ["deps.get"]]
  end
end
