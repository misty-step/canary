defmodule Canary.Schemas.IncidentSignal do
  use Ecto.Schema
  import Ecto.Changeset

  alias Canary.Schemas.Incident

  schema "incident_signals" do
    belongs_to :incident, Incident, type: :string
    field :signal_type, :string
    field :signal_ref, :string
    field :attached_at, :string
    field :resolved_at, :string
  end

  @required ~w(incident_id signal_type signal_ref attached_at)a
  @optional ~w(resolved_at)a
  @signal_types ~w(health_transition error_group)

  def changeset(signal, attrs) do
    signal
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_inclusion(:signal_type, @signal_types)
  end
end
