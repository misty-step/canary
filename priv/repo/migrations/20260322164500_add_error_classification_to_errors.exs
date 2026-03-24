defmodule Canary.Repo.Migrations.AddErrorClassificationToErrors do
  use Ecto.Migration

  def change do
    alter table(:errors) do
      add :classification_category, :string
      add :classification_persistence, :string
      add :classification_component, :string
    end
  end
end
