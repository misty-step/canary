import Config

if System.get_env("PHX_SERVER") do
  config :canary_triage, CanaryTriageWeb.Endpoint, server: true
end

port = String.to_integer(System.get_env("PORT", "4001"))

config :canary_triage, CanaryTriageWeb.Endpoint, http: [port: port]

if config_env() == :prod do
  secret_key_base =
    System.get_env("SECRET_KEY_BASE") ||
      raise "SECRET_KEY_BASE is required"

  host = System.get_env("PHX_HOST", "canary-triage.fly.dev")

  config :canary_triage, CanaryTriageWeb.Endpoint,
    url: [host: host, port: 443, scheme: "https"],
    http: [ip: {0, 0, 0, 0, 0, 0, 0, 0}, port: port],
    secret_key_base: secret_key_base

  # Service -> GitHub repo mapping (JSON string from env)
  service_repos =
    case System.get_env("SERVICE_REPOS") do
      nil -> %{}
      json -> Jason.decode!(json)
    end

  config :canary_triage,
    webhook_secret: System.fetch_env!("CANARY_WEBHOOK_SECRET"),
    canary_endpoint: System.get_env("CANARY_ENDPOINT", "https://canary-obs.fly.dev"),
    canary_api_key: System.fetch_env!("CANARY_API_KEY"),
    github_token: System.fetch_env!("GITHUB_TOKEN"),
    github_org: System.get_env("GITHUB_ORG", "misty-step"),
    openrouter_api_key: System.fetch_env!("OPENROUTER_API_KEY"),
    service_repos: service_repos
end
