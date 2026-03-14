import Config

config :canary, Canary.Repo,
  database: Path.expand("../canary_dev.db", __DIR__),
  pool_size: 1,
  show_sensitive_data_on_connection_error: true

config :canary, Canary.ReadRepo,
  database: Path.expand("../canary_dev.db", __DIR__),
  pool_size: 4

config :canary, CanaryWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4000],
  check_origin: false,
  code_reloader: true,
  debug_errors: true,
  secret_key_base: "dev-only-secret-key-base-that-is-at-least-64-bytes-long-for-phoenix",
  watchers: []

config :canary, dev_routes: true

config :logger, :default_formatter, format: "[$level] $message\n"

config :phoenix, :stacktrace_depth, 20
config :phoenix, :plug_init_mode, :runtime
