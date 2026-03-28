defmodule Canary.Repo.Migrations.AddTargetsServiceAndServiceEvents do
  use Ecto.Migration

  def up do
    alter table(:targets) do
      add :service, :text
    end

    execute("UPDATE targets SET service = name WHERE service IS NULL OR service = ''")

    create index(:targets, [:service])

    create table(:service_events, primary_key: false) do
      add :id, :text, primary_key: true
      add :service, :text, null: false
      add :event, :text, null: false
      add :entity_type, :text, null: false
      add :entity_ref, :text
      add :severity, :text
      add :summary, :text, null: false
      add :payload, :text, null: false
      add :created_at, :text, null: false
    end

    create index(:service_events, [:service, :created_at, :id])
    create index(:service_events, [:created_at, :id])
  end

  def down do
    drop_if_exists index(:service_events, [:created_at, :id])
    drop_if_exists index(:service_events, [:service, :created_at, :id])
    drop table(:service_events)
    drop_if_exists index(:targets, [:service])

    alter table(:targets) do
      remove :service
    end
  end
end
