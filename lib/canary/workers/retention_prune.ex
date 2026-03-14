defmodule Canary.Workers.RetentionPrune do
  @moduledoc """
  Daily Oban worker that prunes old errors and target_checks
  based on configured retention periods. Paginated deletes.
  """

  use Oban.Worker, queue: :maintenance, max_attempts: 3

  alias Canary.Repo

  require Logger

  @batch_size 1_000

  @impl Oban.Worker
  def perform(_job) do
    error_days = Application.get_env(:canary, :error_retention_days, 30)
    check_days = Application.get_env(:canary, :check_retention_days, 7)

    error_cutoff =
      DateTime.utc_now()
      |> DateTime.add(-error_days, :day)
      |> DateTime.to_iso8601()

    check_cutoff =
      DateTime.utc_now()
      |> DateTime.add(-check_days, :day)
      |> DateTime.to_iso8601()

    errors_deleted = prune_table("errors", "created_at", error_cutoff)
    checks_deleted = prune_table("target_checks", "checked_at", check_cutoff)

    Logger.info("Retention prune: #{errors_deleted} errors, #{checks_deleted} checks deleted")
    :ok
  end

  defp prune_table(table, column, cutoff) do
    prune_batch(table, column, cutoff, 0)
  end

  defp prune_batch(table, column, cutoff, total) do
    {deleted, _} =
      Repo.query!(
        "DELETE FROM #{table} WHERE rowid IN (SELECT rowid FROM #{table} WHERE #{column} < ?1 LIMIT ?2)",
        [cutoff, @batch_size]
      )
      |> then(fn %{num_rows: n} -> {n, nil} end)

    new_total = total + deleted

    if deleted >= @batch_size do
      prune_batch(table, column, cutoff, new_total)
    else
      new_total
    end
  end
end
