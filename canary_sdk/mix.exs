defmodule CanarySdk.MixProject do
  use Mix.Project

  @version "0.1.0"
  @source_url "https://github.com/misty-step/canary"

  def project do
    [
      app: :canary_sdk,
      version: @version,
      elixir: "~> 1.17",
      start_permanent: Mix.env() == :prod,
      deps: deps(),
      description: "Elixir SDK for Canary error reporting — Logger handler with async HTTP",
      package: package(),
      source_url: @source_url
    ]
  end

  def application do
    [extra_applications: [:logger]]
  end

  defp deps do
    [
      {:req, "~> 0.5"},
      {:bypass, "~> 2.1", only: :test},
      {:ex_doc, "~> 0.34", only: :dev, runtime: false}
    ]
  end

  defp package do
    [
      licenses: ["MIT"],
      links: %{"GitHub" => @source_url},
      files: ~w(lib mix.exs LICENSE)
    ]
  end
end
