import Config

config :canary_triage,
  service_repos: %{},
  canary_endpoint: "https://canary-obs.fly.dev",
  github_org: "misty-step",
  github_req_options: [finch: CanaryTriage.Finch]

config :canary_triage, CanaryTriageWeb.Endpoint,
  url: [host: "localhost"],
  adapter: Bandit.PhoenixAdapter,
  render_errors: [
    formats: [json: CanaryTriageWeb.ErrorJSON],
    layout: false
  ],
  pubsub_server: CanaryTriage.PubSub

config :logger, :default_formatter,
  format: "$time $metadata[$level] $message\n",
  metadata: [:request_id]

config :phoenix, :json_library, Jason

import_config "#{config_env()}.exs"
