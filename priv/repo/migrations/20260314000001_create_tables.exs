defmodule Canary.Repo.Migrations.CreateTables do
  use Ecto.Migration

  def change do
    # --- API Keys ---
    create table(:api_keys, primary_key: false) do
      add :id, :string, primary_key: true
      add :name, :string, null: false
      add :key_prefix, :string, null: false
      add :key_hash, :string, null: false
      add :created_at, :string, null: false
      add :revoked_at, :string
    end

    # --- Errors ---
    create table(:errors, primary_key: false) do
      add :id, :string, primary_key: true
      add :service, :string, null: false
      add :error_class, :string, null: false
      add :message, :string, null: false
      add :message_template, :string
      add :stack_trace, :text
      add :context, :text
      add :severity, :string, default: "error"
      add :environment, :string, default: "production"
      add :group_hash, :string, null: false
      add :fingerprint, :string
      add :region, :string
      add :created_at, :string, null: false
    end

    create index(:errors, [:service, :created_at])
    create index(:errors, [:group_hash, :created_at])

    # --- Error Groups ---
    create table(:error_groups, primary_key: false) do
      add :group_hash, :string, primary_key: true
      add :service, :string, null: false
      add :error_class, :string, null: false
      add :message_template, :string
      add :severity, :string, null: false
      add :first_seen_at, :string, null: false
      add :last_seen_at, :string, null: false
      add :total_count, :integer, null: false, default: 1
      add :last_error_id, :string, null: false
      add :status, :string, default: "active"
    end

    create index(:error_groups, [:service, :last_seen_at])

    # --- Health Check Targets ---
    create table(:targets, primary_key: false) do
      add :id, :string, primary_key: true
      add :url, :string, null: false
      add :name, :string, null: false
      add :method, :string, default: "GET"
      add :headers, :text
      add :interval_ms, :integer, default: 60_000
      add :timeout_ms, :integer, default: 10_000
      add :expected_status, :string, default: "200"
      add :body_contains, :string
      add :degraded_after, :integer, default: 1
      add :down_after, :integer, default: 3
      add :up_after, :integer, default: 1
      add :active, :integer, default: 1
      add :created_at, :string, null: false
    end

    # --- Target Checks ---
    create table(:target_checks) do
      add :target_id, :string, null: false
      add :checked_at, :string, null: false
      add :status_code, :integer
      add :latency_ms, :integer
      add :result, :string, null: false
      add :tls_expires_at, :string
      add :error_detail, :string
      add :region, :string
    end

    create index(:target_checks, [:target_id, :checked_at])

    # --- Target State ---
    create table(:target_state, primary_key: false) do
      add :target_id, :string, primary_key: true
      add :state, :string, default: "unknown"
      add :consecutive_failures, :integer, default: 0
      add :consecutive_successes, :integer, default: 0
      add :last_checked_at, :string
      add :last_success_at, :string
      add :last_failure_at, :string
      add :last_transition_at, :string
      add :sequence, :integer, default: 0
    end

    # --- Webhooks ---
    create table(:webhooks, primary_key: false) do
      add :id, :string, primary_key: true
      add :url, :string, null: false
      add :events, :text, null: false
      add :secret, :string, null: false
      add :active, :integer, default: 1
      add :created_at, :string, null: false
    end

    # --- Seed Runs ---
    create table(:seed_runs, primary_key: false) do
      add :seed_name, :string, primary_key: true
      add :applied_at, :string, null: false
    end
  end
end
