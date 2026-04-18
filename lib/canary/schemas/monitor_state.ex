defmodule Canary.Schemas.MonitorState do
  use Ecto.Schema
  import Ecto.Changeset

  @type t :: %__MODULE__{}

  @primary_key {:monitor_id, :string, autogenerate: false}
  schema "monitor_state" do
    field :state, :string, default: "unknown"
    field :last_check_in_status, :string
    field :last_check_in_at, :string
    field :last_success_at, :string
    field :last_failure_at, :string
    field :deadline_at, :string
    field :first_missed_at, :string
    field :last_transition_at, :string
    field :sequence, :integer, default: 0
  end

  @fields ~w(
    state
    last_check_in_status
    last_check_in_at
    last_success_at
    last_failure_at
    deadline_at
    first_missed_at
    last_transition_at
    sequence
  )a

  def changeset(state, attrs) do
    state
    |> cast(attrs, @fields)
  end
end
