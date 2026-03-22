defmodule Canary.Repo.Migrations.AddErrorClassificationToErrors do
  use Ecto.Migration

  def change do
    alter table(:errors) do
      add :classification_category, :string
      add :classification_persistence, :string
      add :classification_component, :string
    end

    create constraint(:errors, :classification_category_valid,
             check:
               "classification_category IS NULL OR classification_category IN ('infrastructure','application','unknown')"
           )

    create constraint(:errors, :classification_persistence_valid,
             check:
               "classification_persistence IS NULL OR classification_persistence IN ('transient','persistent','unknown')"
           )

    create constraint(:errors, :classification_component_valid,
             check:
               "classification_component IS NULL OR classification_component IN ('database','network','runtime','unknown')"
           )
  end
end
