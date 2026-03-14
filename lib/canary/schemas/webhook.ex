defmodule Canary.Schemas.Webhook do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key {:id, :string, autogenerate: false}
  schema "webhooks" do
    field :url, :string
    field :events, :string
    field :secret, :string
    field :active, :integer, default: 1
    field :created_at, :string
  end

  @required ~w(url events secret created_at)a

  def changeset(webhook, attrs) do
    webhook
    |> cast(attrs, @required ++ [:active])
    |> validate_required(@required)
  end

  def active?(%__MODULE__{active: 1}), do: true
  def active?(_), do: false

  def event_list(%__MODULE__{events: json}), do: Jason.decode!(json)

  def subscribes_to?(%__MODULE__{} = webhook, event) do
    event in event_list(webhook)
  end
end
