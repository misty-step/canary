defmodule Canary.Schemas.ErrorGroup do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key {:group_hash, :string, autogenerate: false}
  schema "error_groups" do
    field :service, :string
    field :error_class, :string
    field :message_template, :string
    field :severity, :string
    field :first_seen_at, :string
    field :last_seen_at, :string
    field :total_count, :integer, default: 1
    field :last_error_id, :string
    field :status, :string, default: "active"
  end

  @required ~w(service error_class severity first_seen_at last_seen_at last_error_id)a
  @optional ~w(message_template total_count status)a

  def changeset(group, attrs) do
    group
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
  end
end
