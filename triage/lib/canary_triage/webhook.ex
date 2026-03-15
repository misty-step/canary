defmodule CanaryTriage.Webhook do
  @moduledoc """
  HMAC signature verification for Canary webhook payloads.
  """

  @spec verify(binary(), binary(), binary()) :: :ok | {:error, :invalid_signature}
  def verify(body, secret, signature) do
    expected = "sha256=" <> (:crypto.mac(:hmac, :sha256, secret, body) |> Base.encode16(case: :lower))

    if Plug.Crypto.secure_compare(expected, signature) do
      :ok
    else
      {:error, :invalid_signature}
    end
  end
end
