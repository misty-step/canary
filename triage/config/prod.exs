import Config

config :canary_triage, CanaryTriageWeb.Endpoint,
  force_ssl: [
    rewrite_on: [:x_forwarded_proto],
    exclude: [paths: ["/healthz"]]
  ]

config :logger, level: :info
