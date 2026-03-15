import Config

config :canary_triage, CanaryTriageWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4003],
  secret_key_base: "test-only-canary-triage-secret-that-is-at-least-64-bytes-long-for-phoenix",
  server: false

config :canary_triage,
  webhook_secret: "test-secret",
  github_token: "test-token",
  github_req_options: [plug: {Req.Test, CanaryTriage.GitHub}]

config :logger, level: :warning
config :phoenix, :plug_init_mode, :runtime
