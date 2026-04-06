defmodule Canary.Schemas.WebhookDeliveryLedger do
  use Ecto.Schema
  import Ecto.Changeset

  @type t :: %__MODULE__{}

  @statuses ~w(pending retrying delivered discarded suppressed)

  @primary_key {:delivery_id, :string, autogenerate: false}
  schema "webhook_deliveries" do
    field :webhook_id, :string
    field :event, :string
    field :status, :string
    field :attempt_count, :integer, default: 0
    field :reason, :string
    field :first_attempt_at, :string
    field :last_attempt_at, :string
    field :delivered_at, :string
    field :discarded_at, :string
    field :created_at, :string
    field :updated_at, :string
  end

  @required ~w(webhook_id event status attempt_count created_at updated_at)a
  @optional ~w(
    reason
    first_attempt_at
    last_attempt_at
    delivered_at
    discarded_at
  )a

  def changeset(ledger, attrs) do
    ledger
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_number(:attempt_count, greater_than_or_equal_to: 0)
    |> validate_inclusion(:status, @statuses)
  end
end
