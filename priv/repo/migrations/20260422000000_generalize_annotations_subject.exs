defmodule Canary.Repo.Migrations.GeneralizeAnnotationsSubject do
  use Ecto.Migration

  def up do
    alter table(:annotations) do
      add :subject_type, :string
      add :subject_id, :string
    end

    flush()

    execute("""
    UPDATE annotations
    SET subject_type = 'incident', subject_id = incident_id
    WHERE incident_id IS NOT NULL AND (subject_type IS NULL OR subject_id IS NULL)
    """)

    execute("""
    UPDATE annotations
    SET subject_type = 'error_group', subject_id = group_hash
    WHERE group_hash IS NOT NULL
      AND incident_id IS NULL
      AND (subject_type IS NULL OR subject_id IS NULL)
    """)

    create index(:annotations, [:subject_type, :subject_id, :created_at])
    create unique_index(:annotations, [:subject_type, :subject_id, :id])
  end

  def down do
    drop unique_index(:annotations, [:subject_type, :subject_id, :id])
    drop index(:annotations, [:subject_type, :subject_id, :created_at])

    alter table(:annotations) do
      remove :subject_type
      remove :subject_id
    end
  end
end
