defmodule Canary.Repo.Migrations.CreateObanJobs do
  use Ecto.Migration

  def change do
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
      inserted_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
      scheduled_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
      attempted_at TEXT,
      attempted_by TEXT NOT NULL DEFAULT '[]',
      cancelled_at TEXT,
      completed_at TEXT,
      discarded_at TEXT
    )
    """

    execute """
    CREATE INDEX IF NOT EXISTS oban_jobs_state_queue_index
    ON oban_jobs(state, queue, priority, scheduled_at, id)
    """
  end
end
