defmodule Canary.Schemas.ApiKey do
  use Ecto.Schema
  import Ecto.Changeset

  @type t :: %__MODULE__{}

  @primary_key {:id, :string, autogenerate: false}
  schema "api_keys" do
    field :name, :string
    field :key_prefix, :string
    field :key_hash, :string
    field :created_at, :string
    field :revoked_at, :string
  end

  @required ~w(name key_prefix key_hash created_at)a

  def changeset(key, attrs) do
    key
    |> cast(attrs, @required ++ [:revoked_at])
    |> validate_required(@required)
  end

  def active?(%__MODULE__{revoked_at: nil}), do: true
  def active?(_), do: false
end
