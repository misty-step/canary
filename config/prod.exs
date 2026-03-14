import Config

config :canary, CanaryWeb.Endpoint,
  force_ssl: [
    rewrite_on: [:x_forwarded_proto],
    exclude: [paths: ["/healthz", "/readyz"]]
  ]

config :logger, level: :info,
  default_formatter: [
    format: {Canary.JSONLogger, :format},
    metadata: [:request_id, :service, :target, :event]
  ]
