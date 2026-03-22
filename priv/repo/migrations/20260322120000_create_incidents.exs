defmodule Canary.Repo.Migrations.CreateIncidents do
  use Ecto.Migration

  def change do
    create table(:incidents, primary_key: false) do
      add :id, :string, primary_key: true
      add :service, :string, null: false
      add :state, :string, null: false, default: "investigating"
      add :severity, :string, null: false, default: "medium"
      add :title, :string
      add :opened_at, :string, null: false
      add :resolved_at, :string
    end

    create index(:incidents, [:service, :state])
    create index(:incidents, [:opened_at])

    create unique_index(:incidents, [:service],
             where: "state != 'resolved'",
             name: :incidents_open_service_unique_index
           )

    create table(:incident_signals) do
      add :incident_id, references(:incidents, type: :string, on_delete: :delete_all), null: false
      add :signal_type, :string, null: false
      add :signal_ref, :string, null: false
      add :attached_at, :string, null: false
      add :resolved_at, :string
    end

    create index(:incident_signals, [:incident_id])
    create unique_index(:incident_signals, [:incident_id, :signal_type, :signal_ref])
  end
end
