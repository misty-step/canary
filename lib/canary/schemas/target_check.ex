defmodule Canary.Schemas.TargetCheck do
  use Ecto.Schema
  import Ecto.Changeset

  schema "target_checks" do
    field :target_id, :string
    field :checked_at, :string
    field :status_code, :integer
    field :latency_ms, :integer
    field :result, :string
    field :tls_expires_at, :string
    field :error_detail, :string
    field :region, :string
  end

  @required ~w(target_id checked_at result)a
  @optional ~w(status_code latency_ms tls_expires_at error_detail region)a
  @results ~w(success timeout dns_error tls_error status_mismatch body_mismatch connection_error redirect_not_followed)

  def changeset(check, attrs) do
    check
    |> cast(attrs, @required ++ @optional)
    |> validate_required(@required)
    |> validate_inclusion(:result, @results)
  end
end
