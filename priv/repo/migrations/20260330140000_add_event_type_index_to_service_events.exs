defmodule Canary.Repo.Migrations.AddEventTypeIndexToServiceEvents do
  use Ecto.Migration

  def change do
    create index(:service_events, [:event, :created_at, :id])
  end
end
