defmodule CanarySdk do
  @moduledoc """
  Elixir SDK for Canary error reporting.

  Attaches a `:logger` handler that captures error-level events
  and reports them to a Canary instance via async HTTP POST.

  ## Usage

      CanarySdk.attach(
        endpoint: "https://canary-obs.fly.dev",
        api_key: System.fetch_env!("CANARY_API_KEY"),
        service: "my-app"
      )
  """

  @handler_id :canary_sdk

  @spec attach(keyword()) :: :ok | {:error, term()}
  def attach(opts) do
    config = %{
      endpoint: Keyword.fetch!(opts, :endpoint),
      api_key: Keyword.fetch!(opts, :api_key),
      service: Keyword.fetch!(opts, :service),
      environment: Keyword.get(opts, :environment, "production")
    }

    case :logger.add_handler(@handler_id, CanarySdk.Handler, %{config: config}) do
      :ok -> :ok
      {:error, {:already_exist, _}} -> :ok
      {:error, reason} -> {:error, reason}
    end
  end

  @spec detach() :: :ok
  def detach do
    _ = :logger.remove_handler(@handler_id)
    :ok
  end
end
