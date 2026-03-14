defmodule Canary.Seeds do
  @moduledoc "First-boot seed: creates initial targets and a bootstrap API key."

  alias Canary.{Repo, Auth, ID}
  alias Canary.Schemas.Target

  require Logger

  @targets [
    %{name: "cadence-api", url: "https://cadence.fly.dev/api/health", interval_ms: 60_000},
    %{name: "heartbeat", url: "https://heartbeat.fly.dev/api/health", interval_ms: 60_000},
    %{name: "overmind", url: "https://overmind.fly.dev/api/health", interval_ms: 120_000}
  ]

  def run do
    Logger.info("Running initial seeds...")
    now = DateTime.utc_now() |> DateTime.to_iso8601()

    Enum.each(@targets, fn t ->
      attrs = %{
        id: ID.target_id(),
        name: t.name,
        url: t.url,
        interval_ms: t.interval_ms,
        method: "GET",
        expected_status: "200",
        degraded_after: 1,
        down_after: 3,
        up_after: 1,
        active: 1,
        created_at: now
      }

      id = attrs.id
      case %Target{id: id} |> Target.changeset(Map.delete(attrs, :id)) |> Repo.insert(on_conflict: :nothing) do
        {:ok, _} -> Logger.info("Seeded target: #{t.name}")
        _ -> :ok
      end
    end)

    case Auth.generate_key("bootstrap") do
      {:ok, _key, raw_key} ->
        Logger.info("Bootstrap API key generated: #{raw_key}")
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
