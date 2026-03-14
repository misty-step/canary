defmodule Canary.Repo.Migrations.CreateObanTables do
  use Ecto.Migration

  def up do
    # Oban Lite engine tables — created manually because
    # Oban.Engines.Lite expects them to exist before it starts
    execute """
    CREATE TABLE IF NOT EXISTS oban_jobs (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      state TEXT NOT NULL DEFAULT 'available',
      queue TEXT NOT NULL DEFAULT 'default',
      worker TEXT NOT NULL,
      args TEXT NOT NULL DEFAULT '{}',
      meta TEXT NOT NULL DEFAULT '{}',
      tags TEXT NOT NULL DEFAULT '[]',
      errors TEXT NOT NULL DEFAULT '[]',
      attempt INTEGER NOT NULL DEFAULT 0,
      max_attempts INTEGER NOT NULL DEFAULT 20,
      priority INTEGER NOT NULL DEFAULT 0,
      attempted_at TEXT,
      completed_at TEXT,
      cancelled_at TEXT,
      discarded_at TEXT,
      inserted_at TEXT NOT NULL DEFAULT (datetime('now')),
      scheduled_at TEXT NOT NULL DEFAULT (datetime('now'))
    )
    """

    execute "CREATE INDEX IF NOT EXISTS oban_jobs_queue_state_index ON oban_jobs(queue, state, scheduled_at)"
    execute "CREATE INDEX IF NOT EXISTS oban_jobs_state_index ON oban_jobs(state)"
  end

  def down do
    execute "DROP TABLE IF EXISTS oban_jobs"
  end
end
