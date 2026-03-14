defmodule Canary.Schemas.Target do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key {:id, :string, autogenerate: false}
  schema "targets" do
    field :url, :string
    field :name, :string
    field :method, :string, default: "GET"
    field :headers, :string
    field :interval_ms, :integer, default: 60_000
    field :timeout_ms, :integer, default: 10_000
    field :expected_status, :string, default: "200"
    field :body_contains, :string
    field :degraded_after, :integer, default: 1
    field :down_after, :integer, default: 3
    field :up_after, :integer, default: 1
    field :active, :integer, default: 1
    field :created_at, :string
  end

  @required ~w(url name created_at)a
  @optional ~w(method headers interval_ms timeout_ms expected_status body_contains degraded_after down_after up_after active)a

  def changeset(target, attrs) do
    target
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_inclusion(:method, ["GET", "HEAD"])
    |> validate_number(:interval_ms, greater_than: 0)
    |> validate_number(:timeout_ms, greater_than: 0)
  end

  def active?(%__MODULE__{active: 1}), do: true
  def active?(_), do: false

  def parsed_headers(%__MODULE__{headers: nil}), do: %{}
  def parsed_headers(%__MODULE__{headers: json}), do: Jason.decode!(json)
end
