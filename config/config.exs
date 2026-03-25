import Config

config :canary,
  env: config_env(),
  ecto_repos: [Canary.Repo],
  error_retention_days: 30,
  check_retention_days: 7,
  allow_private_targets: false,
  in_project_module_prefixes: [],
  in_project_path_prefixes: []

config :canary, Canary.Repo, adapter: Ecto.Adapters.SQLite3

config :canary, Canary.ReadRepo, adapter: Ecto.Adapters.SQLite3

config :canary, CanaryWeb.Endpoint,
  url: [host: "localhost"],
  adapter: Bandit.PhoenixAdapter,
  render_errors: [
    formats: [json: CanaryWeb.ErrorJSON],
    layout: false
  ],
  pubsub_server: Canary.PubSub,
  live_view: [signing_salt: "canary_lv_salt"]

config :canary, Oban,
  engine: Oban.Engines.Lite,
  repo: Canary.Repo,
  queues: [webhooks: 10, maintenance: 2],
  plugins: [
    {Oban.Plugins.Cron,
     crontab: [
       {"0 3 * * *", Canary.Workers.RetentionPrune},
       {"0 6 * * *", Canary.Workers.TlsScan}
     ]}
  ]

config :logger, :default_formatter,
  format: "$time $metadata[$level] $message\n",
  metadata: [:request_id, :target, :event]

config :phoenix, :json_library, Jason

config :mime, :types, %{
  "text/csv" => ["csv"]
}

import_config "#{config_env()}.exs"
