defmodule Canary.Alerter.SignerTest do
  use ExUnit.Case, async: true

  alias Canary.Alerter.Signer

  @secret "test-webhook-secret"
  @body ~s({"event":"error.new_class","data":"test"})

  test "sign/2 returns hex-encoded HMAC-SHA256" do
    signature = Signer.sign(@body, @secret)

    expected =
      :crypto.mac(:hmac, :sha256, @secret, @body)
      |> Base.encode16(case: :lower)

    assert signature == expected
    assert byte_size(signature) == 64
  end

  test "signature_header/2 prefixes with sha256=" do
    header = Signer.signature_header(@body, @secret)
    assert "sha256=" <> hex = header
    assert byte_size(hex) == 64
  end

  test "verify/3 returns true for valid signature" do
    header = Signer.signature_header(@body, @secret)
    assert Signer.verify(@body, @secret, header)
  end

  test "verify/3 returns false for wrong secret" do
    header = Signer.signature_header(@body, @secret)
    refute Signer.verify(@body, "wrong-secret", header)
  end

  test "verify/3 returns false for tampered body" do
    header = Signer.signature_header(@body, @secret)
    refute Signer.verify(@body <> "tampered", @secret, header)
  end
end
