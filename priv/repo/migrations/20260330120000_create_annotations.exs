defmodule Canary.Repo.Migrations.CreateAnnotations do
  use Ecto.Migration

  def change do
    create table(:annotations, primary_key: false) do
      add :id, :string, primary_key: true
      add :incident_id, references(:incidents, type: :string, on_delete: :delete_all)
      add :group_hash, :string
      add :agent, :string, null: false
      add :action, :string, null: false
      add :metadata, :text
      add :created_at, :string, null: false
    end

    create index(:annotations, [:incident_id, :action])
    create index(:annotations, [:group_hash, :action])
    create index(:annotations, [:action])
  end
end
