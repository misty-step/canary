import Config

config :canary, Canary.Repo,
  database: Path.expand("../canary_test#{System.get_env("MIX_TEST_PARTITION")}.db", __DIR__),
  pool_size: 1,
  pool: Ecto.Adapters.SQL.Sandbox

config :canary, Canary.ReadRepo,
  database: Path.expand("../canary_test#{System.get_env("MIX_TEST_PARTITION")}.db", __DIR__),
  pool_size: 1,
  pool: Ecto.Adapters.SQL.Sandbox

config :canary, CanaryWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4002],
  secret_key_base: "test-only-secret-key-base-that-is-at-least-64-bytes-long-for-phoenix",
  server: false

config :canary, Oban, testing: :inline

config :canary, run_migrations: false

config :bcrypt_elixir, :log_rounds, 4

config :logger, level: :warning

config :phoenix, :plug_init_mode, :runtime
