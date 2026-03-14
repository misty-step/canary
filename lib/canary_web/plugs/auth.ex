defmodule CanaryWeb.Plugs.Auth do
  @moduledoc "API key authentication plug."

  import Plug.Conn
  alias Canary.Auth
  alias Canary.Errors.RateLimiter

  def init(opts), do: opts

  def call(conn, _opts) do
    case extract_key(conn) do
      {:ok, raw_key} ->
        case Auth.verify_key(raw_key) do
          {:ok, api_key} ->
            assign(conn, :api_key, api_key)

          {:error, :invalid} ->
            ip = to_string(:inet.ntoa(conn.remote_ip))
            RateLimiter.check(ip, :auth_fail)

            CanaryWeb.Plugs.ProblemDetails.render_error(
              conn, 401, "invalid_api_key", "Invalid or revoked API key."
            )
        end

      :error ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn, 401, "invalid_api_key", "Missing Authorization header. Use: Bearer sk_..."
        )
    end
  end

  defp extract_key(conn) do
    case get_req_header(conn, "authorization") do
      ["Bearer " <> key] -> {:ok, String.trim(key)}
      _ -> :error
    end
  end
end
