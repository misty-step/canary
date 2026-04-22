defmodule Canary.Schemas.Annotation do
  use Ecto.Schema
  import Ecto.Changeset

  @type t :: %__MODULE__{}

  @subject_types ~w(incident error_group target monitor)
  def subject_types, do: @subject_types

  @primary_key {:id, :string, autogenerate: false}
  schema "annotations" do
    field :subject_type, :string
    field :subject_id, :string
    field :incident_id, :string
    field :group_hash, :string
    field :agent, :string
    field :action, :string
    field :metadata, :string
    field :created_at, :string
  end

  @required ~w(subject_type subject_id agent action created_at)a
  @optional ~w(incident_id group_hash metadata)a

  def changeset(annotation, attrs) do
    annotation
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_inclusion(:subject_type, @subject_types)
  end
end
