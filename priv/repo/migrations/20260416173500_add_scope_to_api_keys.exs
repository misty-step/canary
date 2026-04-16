defmodule Canary.Repo.Migrations.AddScopeToApiKeys do
  use Ecto.Migration

  def up do
    alter table(:api_keys) do
      add :scope, :string, null: false, default: "admin"
    end

    execute("UPDATE api_keys SET scope = 'admin' WHERE scope IS NULL")
  end

  def down do
    alter table(:api_keys) do
      remove :scope
    end
  end
end
