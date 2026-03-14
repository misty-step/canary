defmodule Canary.Schemas.Error do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key {:id, :string, autogenerate: false}
  schema "errors" do
    field :service, :string
    field :error_class, :string
    field :message, :string
    field :message_template, :string
    field :stack_trace, :string
    field :context, :string
    field :severity, :string, default: "error"
    field :environment, :string, default: "production"
    field :group_hash, :string
    field :fingerprint, :string
    field :region, :string
    field :created_at, :string
  end

  @required ~w(service error_class message group_hash created_at)a
  @optional ~w(message_template stack_trace context severity environment fingerprint region)a
  @severities ~w(error warning info)

  def changeset(error, attrs) do
    error
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_inclusion(:severity, @severities)
    |> validate_length(:message, max: 4_096)
    |> validate_length(:stack_trace, max: 32_768)
  end
end
