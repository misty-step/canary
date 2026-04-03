defmodule Canary.Repo.Migrations.CreateWebhookDeliveries do
  use Ecto.Migration

  def change do
    create table(:webhook_deliveries, primary_key: false) do
      add :delivery_id, :string, primary_key: true
      add :webhook_id, :string, null: false
      add :event, :string, null: false
      add :status, :string, null: false
      add :attempt_count, :integer, null: false, default: 0
      add :reason, :string
      add :first_attempt_at, :string
      add :last_attempt_at, :string
      add :delivered_at, :string
      add :discarded_at, :string
      add :created_at, :string, null: false
      add :updated_at, :string, null: false
    end

    create index(:webhook_deliveries, [:webhook_id, :created_at])
    create index(:webhook_deliveries, [:event, :created_at])
    create index(:webhook_deliveries, [:status, :created_at])
  end
end
