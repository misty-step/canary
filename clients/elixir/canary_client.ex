defmodule CanaryClient do
  @moduledoc """
  Canary Elixir client — thin HTTP wrapper for error reporting.

  ## Configuration

      config :canary_client,
        endpoint: "https://canary-obs.fly.dev",
        api_key: System.get_env("CANARY_API_KEY"),
        service: "my-service"

  ## Usage

      # Explicit capture
      CanaryClient.capture(%RuntimeError{message: "boom"})
      CanaryClient.capture(error, stacktrace, context: %{user_id: id})

      # With fingerprint override
      CanaryClient.capture(error, nil, fingerprint: ["payment-flow", "stripe"])
  """

  @default_timeout 5_000

  @spec capture(Exception.t() | binary(), list() | nil, keyword()) ::
          {:ok, map()} | {:error, term()}
  def capture(error, stacktrace \\ nil, opts \\ []) do
    unless enabled?() do
      {:ok, :disabled}
    else
      {error_class, message} = normalize_error(error)
      stack_trace = format_stacktrace(stacktrace)

      body = %{
        service: config(:service),
        error_class: error_class,
        message: message,
        stack_trace: stack_trace,
        severity: Keyword.get(opts, :severity, "error"),
        environment: config(:environment, "production"),
        context: opts |> Keyword.get(:context) |> encode_context(),
        fingerprint: Keyword.get(opts, :fingerprint)
      }
      |> Enum.reject(fn {_k, v} -> is_nil(v) end)
      |> Map.new()

      post("/api/v1/errors", body)
    end
  rescue
    # Never crash from error reporting
    e -> {:error, e}
  end

  defp post(path, body) do
    url = "#{config(:endpoint)}#{path}"

    case Req.post(url,
           json: body,
           headers: [{"authorization", "Bearer #{config(:api_key)}"}],
           receive_timeout: @default_timeout,
           retry: false
         ) do
      {:ok, %{status: status, body: resp}} when status in 200..299 ->
        {:ok, resp}

      {:ok, %{status: status, body: resp}} ->
        {:error, {status, resp}}

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp normalize_error(%{__struct__: mod} = error) do
    class = mod |> Module.split() |> Enum.join(".")
    {class, Exception.message(error)}
  end

  defp normalize_error(message) when is_binary(message) do
    {"StringError", message}
  end

  defp normalize_error(other) do
    {"UnknownError", inspect(other)}
  end

  defp format_stacktrace(nil), do: nil

  defp format_stacktrace(stacktrace) when is_list(stacktrace) do
    Exception.format_stacktrace(stacktrace)
  end

  defp format_stacktrace(stacktrace) when is_binary(stacktrace), do: stacktrace

  defp encode_context(nil), do: nil

  defp encode_context(ctx) when is_map(ctx) do
    json = Jason.encode!(ctx)
    if byte_size(json) > 8_192, do: nil, else: ctx
  end

  defp config(key, default \\ nil) do
    Application.get_env(:canary_client, key, default)
  end

  defp enabled? do
    config(:enabled, true) and not is_nil(config(:api_key)) and not is_nil(config(:endpoint))
  end
end
