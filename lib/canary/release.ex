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
  rescue
    e -> Logger.warning("Migration skipped: #{inspect(e)}")
  end

  defp run_seeds do
    case Canary.Repo.query(
           "SELECT seed_name FROM seed_runs WHERE seed_name = 'initial_config_v1'"
         ) do
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
