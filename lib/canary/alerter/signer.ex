defmodule Canary.Alerter.Signer do
  @moduledoc "HMAC-SHA256 webhook payload signing."

  @spec sign(binary(), binary()) :: binary()
  def sign(body, secret) do
    :crypto.mac(:hmac, :sha256, secret, body)
    |> Base.encode16(case: :lower)
  end

  @spec signature_header(binary(), binary()) :: binary()
  def signature_header(body, secret) do
    "sha256=#{sign(body, secret)}"
  end

  @spec verify(binary(), binary(), binary()) :: boolean()
  def verify(body, secret, signature) do
    expected = signature_header(body, secret)
    Plug.Crypto.secure_compare(expected, signature)
  end
end
