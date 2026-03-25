defmodule Canary.Repo.Migrations.AddErrorSearchFts do
  use Ecto.Migration

  def up do
    execute """
    CREATE VIRTUAL TABLE errors_fts USING fts5(
      service,
      error_class,
      message,
      stack_trace,
      content='errors',
      content_rowid='rowid'
    )
    """

    execute """
    CREATE TRIGGER errors_fts_insert
    AFTER INSERT ON errors
    BEGIN
      INSERT INTO errors_fts(rowid, service, error_class, message, stack_trace)
      VALUES (new.rowid, new.service, new.error_class, new.message, new.stack_trace);
    END
    """

    execute """
    CREATE TRIGGER errors_fts_delete
    AFTER DELETE ON errors
    BEGIN
      INSERT INTO errors_fts(errors_fts, rowid, service, error_class, message, stack_trace)
      VALUES ('delete', old.rowid, old.service, old.error_class, old.message, old.stack_trace);
    END
    """

    execute """
    CREATE TRIGGER errors_fts_update
    AFTER UPDATE ON errors
    BEGIN
      INSERT INTO errors_fts(errors_fts, rowid, service, error_class, message, stack_trace)
      VALUES ('delete', old.rowid, old.service, old.error_class, old.message, old.stack_trace);

      INSERT INTO errors_fts(rowid, service, error_class, message, stack_trace)
      VALUES (new.rowid, new.service, new.error_class, new.message, new.stack_trace);
    END
    """

    execute "INSERT INTO errors_fts(errors_fts) VALUES ('rebuild')"
  end

  def down do
    execute "DROP TRIGGER IF EXISTS errors_fts_update"
    execute "DROP TRIGGER IF EXISTS errors_fts_delete"
    execute "DROP TRIGGER IF EXISTS errors_fts_insert"
    execute "DROP TABLE IF EXISTS errors_fts"
  end
end
