defmodule Canary.Schemas.Monitor do
  use Ecto.Schema
  import Ecto.Changeset

  @type t :: %__MODULE__{}

  @modes ~w(schedule ttl)

  @primary_key {:id, :string, autogenerate: false}
  schema "monitors" do
    field :name, :string
    field :service, :string
    field :mode, :string
    field :expected_every_ms, :integer
    field :grace_ms, :integer, default: 0
    field :created_at, :string
  end

  @required ~w(name service mode expected_every_ms created_at)a
  @optional ~w(grace_ms)a

  def changeset(monitor, attrs) do
    monitor
    |> cast(attrs, @required ++ @optional)
    |> put_service_default()
    |> validate_required(@required)
    |> validate_inclusion(:mode, @modes)
    |> validate_number(:expected_every_ms, greater_than: 0)
    |> validate_number(:grace_ms, greater_than_or_equal_to: 0)
    |> unique_constraint(:name)
  end

  def service_name(%__MODULE__{service: service, name: name}) when service in [nil, ""], do: name
  def service_name(%__MODULE__{service: service}), do: service

  defp put_service_default(changeset) do
    case {get_field(changeset, :service), get_field(changeset, :name)} do
      {service, name} when service in [nil, ""] and is_binary(name) and name != "" ->
        put_change(changeset, :service, name)

      _ ->
        changeset
    end
  end
end
