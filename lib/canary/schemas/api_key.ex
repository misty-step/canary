defmodule Canary.Schemas.ApiKey do
  use Ecto.Schema
  import Ecto.Changeset

  @type t :: %__MODULE__{}
  @type permission :: :admin | :ingest | :read

  @scopes ["admin", "ingest-only", "read-only"]

  @primary_key {:id, :string, autogenerate: false}
  schema "api_keys" do
    field :name, :string
    field :scope, :string, default: "admin"
    field :key_prefix, :string
    field :key_hash, :string
    field :created_at, :string
    field :revoked_at, :string
  end

  @required ~w(name scope key_prefix key_hash created_at)a

  def changeset(key, attrs) do
    key
    |> cast(attrs, @required ++ [:revoked_at])
    |> validate_required(@required)
    |> validate_inclusion(:scope, @scopes)
  end

  def scopes, do: @scopes
  def default_scope, do: "admin"
  def ingest_scope, do: "ingest-only"
  def read_scope, do: "read-only"

  def active?(%__MODULE__{revoked_at: nil}), do: true
  def active?(_), do: false

  def allows?(%__MODULE__{scope: scope}, permission) when is_atom(permission) do
    scope in allowed_scopes(permission)
  end

  def allowed_scopes(:admin), do: [default_scope()]
  def allowed_scopes(:ingest), do: [default_scope(), ingest_scope()]
  def allowed_scopes(:read), do: [default_scope(), read_scope()]
  def allowed_scopes(_), do: []
end
