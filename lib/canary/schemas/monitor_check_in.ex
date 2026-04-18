defmodule Canary.Schemas.MonitorCheckIn do
  use Ecto.Schema
  import Ecto.Changeset

  @type t :: %__MODULE__{}

  @statuses ~w(alive in_progress ok error)

  @primary_key {:id, :string, autogenerate: false}
  schema "monitor_check_ins" do
    belongs_to :monitor, Canary.Schemas.Monitor, type: :string
    field :external_id, :string
    field :status, :string
    field :observed_at, :string
    field :ttl_ms, :integer
    field :summary, :string
    field :context, :string
  end

  @required ~w(monitor_id status observed_at)a
  @optional ~w(external_id ttl_ms summary context)a

  def changeset(check_in, attrs) do
    check_in
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_inclusion(:status, @statuses)
    |> validate_number(:ttl_ms, greater_than: 0)
  end
end
