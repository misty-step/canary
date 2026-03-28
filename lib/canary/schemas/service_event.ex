defmodule Canary.Schemas.ServiceEvent do
  use Ecto.Schema
  import Ecto.Changeset

  @entity_types ~w(error_group target incident)
  @events Canary.Webhooks.EventTypes.all()

  @primary_key {:id, :string, autogenerate: false}
  schema "service_events" do
    field :service, :string
    field :event, :string
    field :entity_type, :string
    field :entity_ref, :string
    field :severity, :string
    field :summary, :string
    field :payload, :string
    field :created_at, :string
  end

  @required ~w(service event entity_type summary payload created_at)a
  @optional ~w(entity_ref severity)a

  def with_id(id, attrs) do
    %__MODULE__{id: id}
    |> changeset(attrs)
  end

  def changeset(service_event, attrs) do
    service_event
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_inclusion(:event, @events)
    |> validate_inclusion(:entity_type, @entity_types)
  end
end
