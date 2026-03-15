defmodule CanarySdk.Client do
  @moduledoc false

  @receive_timeout 5_000

  @spec send_error(map(), map()) :: {:ok, term()} | {:error, term()}
  def send_error(config, body) do
    Req.post("#{config.endpoint}/api/v1/errors",
      json: body,
      headers: [{"authorization", "Bearer #{config.api_key}"}],
      receive_timeout: @receive_timeout,
      retry: false
    )
  end
end
