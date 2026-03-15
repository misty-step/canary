defmodule Canary.Seeds do
  @moduledoc """
  First-boot seed: generates a bootstrap API key.
  Targets and webhooks are configured at runtime via API — not hardcoded.
  """

  alias Canary.{Auth, Repo}

  require Logger

  def run do
    Logger.info("Running initial seeds...")
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    case Auth.generate_key("bootstrap") do
      {:ok, _key, raw_key} ->
        Logger.info("Bootstrap API key: #{raw_key}")
        Logger.info("Store this key securely — it will not be shown again.")

      _ ->
        :ok
    end

    Repo.query!(
      "INSERT OR IGNORE INTO seed_runs (seed_name, applied_at) VALUES ('initial_config_v1', ?1)",
      [now]
    )

    :ok
  end
end
