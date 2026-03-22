defmodule Canary.Schemas.Incident do
  use Ecto.Schema
  import Ecto.Changeset

  alias Canary.Schemas.IncidentSignal

  @type t :: %__MODULE__{}

  @primary_key {:id, :string, autogenerate: false}
  schema "incidents" do
    field :service, :string
    field :state, :string, default: "investigating"
    field :severity, :string, default: "medium"
    field :title, :string
    field :opened_at, :string
    field :resolved_at, :string

    has_many :signals, IncidentSignal, foreign_key: :incident_id, references: :id
  end

  @required ~w(service state severity opened_at)a
  @optional ~w(title resolved_at)a
  @states ~w(investigating resolved)
  @severities ~w(medium high)

  def changeset(incident, attrs) do
    incident
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_inclusion(:state, @states)
    |> validate_inclusion(:severity, @severities)
  end
end
