defmodule Canary.Auth do
  @moduledoc """
  API key management: generation, hashing, constant-time validation.
  Keys use format sk_<env>_<nanoid>. Stored as (prefix, bcrypt_hash).
  """

  alias Canary.{ID, Repo}
  alias Canary.Schemas.ApiKey
  import Ecto.Query

  @prefix_len 12

  @spec generate_key(String.t(), String.t(), String.t()) ::
          {:ok, %ApiKey{}, String.t()} | {:error, Ecto.Changeset.t()}
  def generate_key(name, env \\ "live", scope \\ ApiKey.default_scope()) do
    raw_key = "sk_#{env}_#{Nanoid.generate(24)}"
    prefix = String.slice(raw_key, 0, @prefix_len)
    hash = Bcrypt.hash_pwd_salt(raw_key)
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    attrs = %{
      id: ID.key_id(),
      name: name,
      scope: scope,
      key_prefix: prefix,
      key_hash: hash,
      created_at: now
    }

    case %ApiKey{id: attrs.id} |> ApiKey.changeset(Map.delete(attrs, :id)) |> Repo.insert() do
      {:ok, key} -> {:ok, key, raw_key}
      {:error, cs} -> {:error, cs}
    end
  end

  @spec verify_key(String.t()) :: {:ok, %ApiKey{}} | {:error, :invalid}
  def verify_key(raw_key) when is_binary(raw_key) do
    prefix = String.slice(raw_key, 0, @prefix_len)

    query = from k in ApiKey, where: k.key_prefix == ^prefix and is_nil(k.revoked_at)

    case Repo.all(query) do
      [] ->
        Bcrypt.no_user_verify()
        {:error, :invalid}

      candidates ->
        find_matching_key(candidates, raw_key)
    end
  end

  def verify_key(_), do: {:error, :invalid}

  defp find_matching_key(candidates, raw_key) do
    Enum.find_value(candidates, fn key ->
      if Bcrypt.verify_pass(raw_key, key.key_hash), do: {:ok, key}
    end) || {:error, :invalid}
  end

  @spec list_keys() :: [%ApiKey{}]
  def list_keys do
    from(k in ApiKey, order_by: [desc: k.created_at])
    |> Repo.all()
  end

  @spec revoke_key(String.t()) :: {:ok, %ApiKey{}} | {:error, :not_found | Ecto.Changeset.t()}
  def revoke_key(key_id) do
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    case Repo.get(ApiKey, key_id) do
      nil -> {:error, :not_found}
      key -> key |> ApiKey.changeset(%{revoked_at: now}) |> Repo.update()
    end
  end
end
