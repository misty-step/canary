defmodule CanaryWeb.Plugs.RateLimit do
  @moduledoc "Per-API-key rate limiting plug."

  alias Canary.Errors.RateLimiter

  def init(opts), do: opts

  def call(conn, opts) do
    type = Keyword.get(opts, :type, :ingest)

    key =
      case conn.assigns[:api_key] do
        %{id: id} -> id
        _ -> to_string(:inet.ntoa(conn.remote_ip))
      end

    case RateLimiter.check(key, type) do
      :ok ->
        conn

      {:error, retry_after} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          429,
          "rate_limited",
          "Rate limit exceeded. Try again in #{retry_after} seconds.",
          %{retry_after: retry_after}
        )
    end
  end
end
