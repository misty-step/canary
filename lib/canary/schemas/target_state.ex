defmodule Canary.Schemas.TargetState do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key {:target_id, :string, autogenerate: false}
  schema "target_state" do
    field :state, :string, default: "unknown"
    field :consecutive_failures, :integer, default: 0
    field :consecutive_successes, :integer, default: 0
    field :last_checked_at, :string
    field :last_success_at, :string
    field :last_failure_at, :string
    field :last_transition_at, :string
    field :sequence, :integer, default: 0
  end

  @fields ~w(state consecutive_failures consecutive_successes last_checked_at last_success_at last_failure_at last_transition_at sequence)a

  def changeset(state, attrs) do
    state
    |> cast(attrs, @fields)
  end
end
