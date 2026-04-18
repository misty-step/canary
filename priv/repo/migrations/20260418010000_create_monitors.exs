defmodule Canary.Repo.Migrations.CreateMonitors do
  use Ecto.Migration

  def change do
    create table(:monitors, primary_key: false) do
      add :id, :text, primary_key: true
      add :name, :text, null: false
      add :service, :text, null: false
      add :mode, :text, null: false
      add :expected_every_ms, :integer, null: false
      add :grace_ms, :integer, null: false, default: 0
      add :created_at, :text, null: false
    end

    create unique_index(:monitors, [:name])
    create index(:monitors, [:service])

    create table(:monitor_state, primary_key: false) do
      add :monitor_id, references(:monitors, type: :text, on_delete: :delete_all),
        primary_key: true

      add :state, :text, null: false, default: "unknown"
      add :last_check_in_status, :text
      add :last_check_in_at, :text
      add :last_success_at, :text
      add :last_failure_at, :text
      add :deadline_at, :text
      add :first_missed_at, :text
      add :last_transition_at, :text
      add :sequence, :integer, null: false, default: 0
    end

    create table(:monitor_check_ins, primary_key: false) do
      add :id, :text, primary_key: true
      add :monitor_id, references(:monitors, type: :text, on_delete: :delete_all), null: false
      add :external_id, :text
      add :status, :text, null: false
      add :observed_at, :text, null: false
      add :ttl_ms, :integer
      add :summary, :text
      add :context, :text
    end

    create index(:monitor_check_ins, [:monitor_id, :observed_at])
  end
end
