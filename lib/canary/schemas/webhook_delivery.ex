defmodule Canary.Schemas.WebhookDelivery do
  use Ecto.Schema
  import Ecto.Changeset

  @type t :: %__MODULE__{}

  @statuses ~w(queued retrying delivered discarded suppressed)

  @primary_key {:id, :string, autogenerate: false}
  schema "webhook_deliveries" do
    field :webhook_id, :string
    field :event, :string
    field :status, :string
    field :attempt_count, :integer, default: 0
    field :last_status_code, :integer
    field :last_error, :string
    field :suppression_reason, :string
    field :first_attempted_at, :string
    field :last_attempted_at, :string
    field :completed_at, :string
    field :created_at, :string
  end

  @required ~w(webhook_id event status attempt_count created_at)a
  @optional ~w(last_status_code last_error suppression_reason first_attempted_at last_attempted_at completed_at)a

  def changeset(delivery, attrs) do
    delivery
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_inclusion(:status, @statuses)
    |> validate_number(:attempt_count, greater_than_or_equal_to: 0)
  end
end
