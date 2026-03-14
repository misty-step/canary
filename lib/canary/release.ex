defmodule Canary.Release do
  @moduledoc """
  Boot-time task: run migrations and seeds.
  Started as a worker in the supervision tree — runs once then idles.
  Placed after Repo in the children list so the DB is ready.
  """

  use GenServer

  require Logger

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    if Application.get_env(:canary, :run_migrations, true) do
      run_migrations()
      run_seeds()
    end

    {:ok, %{}}
  end

  defp run_migrations do
    path = Application.app_dir(:canary, "priv/repo/migrations")
    Ecto.Migrator.run(Canary.Repo, path, :up, all: true)

    # Oban Lite tables — run raw SQL since Oban.Migrations.SQLite
    # uses Ecto.Migration macros that don't work with Migrator.run
    Canary.Repo.query!("""
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
    """)

    Canary.Repo.query!("""
    CREATE INDEX IF NOT EXISTS oban_jobs_state_queue_index
    ON oban_jobs(state, queue, priority, scheduled_at, id)
    """)
  rescue
    e -> Logger.warning("Migration skipped: #{inspect(e)}")
  end

  defp run_seeds do
    case Canary.Repo.query("SELECT seed_name FROM seed_runs WHERE seed_name = 'initial_config_v1'") do
      {:ok, %{rows: []}} ->
        Canary.Seeds.run()

      {:ok, _} ->
        :ok

      {:error, _} ->
        :ok
    end
  rescue
    _ -> :ok
  end
end
