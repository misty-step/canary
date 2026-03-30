defmodule Canary.Schemas.Annotation do
  use Ecto.Schema
  import Ecto.Changeset

  @type t :: %__MODULE__{}

  @primary_key {:id, :string, autogenerate: false}
  schema "annotations" do
    field :incident_id, :string
    field :group_hash, :string
    field :agent, :string
    field :action, :string
    field :metadata, :string
    field :created_at, :string
  end

  @required ~w(agent action created_at)a
  @optional ~w(incident_id group_hash metadata)a

  def changeset(annotation, attrs) do
    annotation
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_target()
  end

  defp validate_target(changeset) do
    incident_id = get_field(changeset, :incident_id)
    group_hash = get_field(changeset, :group_hash)

    if is_nil(incident_id) and is_nil(group_hash) do
      add_error(changeset, :base, "must target an incident or error group")
    else
      changeset
    end
  end
end
