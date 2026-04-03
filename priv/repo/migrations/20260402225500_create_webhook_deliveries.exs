defmodule Canary.Repo.Migrations.CreateWebhookDeliveries do
  use Ecto.Migration

  def change do
    create table(:webhook_deliveries, primary_key: false) do
      add :id, :string, primary_key: true
      add :webhook_id, :string, null: false
      add :event, :string, null: false
      add :status, :string, null: false
      add :attempt_count, :integer, null: false, default: 0
      add :last_status_code, :integer
      add :last_error, :text
      add :suppression_reason, :string
      add :first_attempted_at, :string
      add :last_attempted_at, :string
      add :completed_at, :string
      add :created_at, :string, null: false
    end

    create index(:webhook_deliveries, [:webhook_id, :created_at, :id])
    create index(:webhook_deliveries, [:status, :created_at])
  end
end
