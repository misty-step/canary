defmodule CanaryTriageWeb.RawBodyReader do
  @moduledoc """
  Caches the raw request body in conn.assigns for HMAC verification.
  Plug.Parsers consumes the body, so we read it first and stash it.
  """

  def read_body(conn, opts) do
    case Plug.Conn.read_body(conn, opts) do
      {:ok, body, conn} ->
        conn = Plug.Conn.assign(conn, :raw_body, body)
        {:ok, body, conn}

      {:more, body, conn} ->
        conn = Plug.Conn.assign(conn, :raw_body, body)
        {:more, body, conn}

      {:error, reason} ->
        {:error, reason}
    end
  end
end
