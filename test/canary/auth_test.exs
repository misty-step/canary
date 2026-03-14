defmodule Canary.AuthTest do
  use Canary.DataCase

  alias Canary.Auth

  describe "generate_key/1" do
    test "creates API key and returns raw key" do
      {:ok, key, raw_key} = Auth.generate_key("test-key")

      assert key.name == "test-key"
      assert String.starts_with?(raw_key, "sk_live_")
      assert String.starts_with?(key.key_prefix, "sk_live_")
      assert key.key_hash != raw_key
      assert is_nil(key.revoked_at)
    end
  end

  describe "verify_key/1" do
    test "verifies valid key" do
      {:ok, _key, raw_key} = Auth.generate_key("test-key")

      assert {:ok, verified} = Auth.verify_key(raw_key)
      assert verified.name == "test-key"
    end

    test "rejects invalid key" do
      assert {:error, :invalid} = Auth.verify_key("sk_live_nonexistent12345678")
    end

    test "rejects revoked key" do
      {:ok, key, raw_key} = Auth.generate_key("test-key")
      {:ok, _} = Auth.revoke_key(key.id)

      assert {:error, :invalid} = Auth.verify_key(raw_key)
    end
  end

  describe "list_keys/0" do
    test "lists all keys" do
      {:ok, _, _} = Auth.generate_key("key-1")
      {:ok, _, _} = Auth.generate_key("key-2")

      keys = Auth.list_keys()
      assert length(keys) >= 2
    end
  end
end
