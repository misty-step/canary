import Config

if System.get_env("PHX_SERVER") do
  config :canary, CanaryWeb.Endpoint, server: true
end

config :canary, CanaryWeb.Endpoint,
  http: [port: String.to_integer(System.get_env("PORT", "4000"))]

if config_env() == :prod do
  db_path = System.get_env("CANARY_DB_PATH", "/data/canary.db")

  config :canary, Canary.Repo,
    database: db_path,
    pool_size: 1

  config :canary, Canary.ReadRepo,
    database: db_path,
    pool_size: 4

  secret_key_base =
    System.get_env("SECRET_KEY_BASE") ||
      raise "SECRET_KEY_BASE is required in production"

  host = System.get_env("PHX_HOST", "canary.fly.dev")

  port = String.to_integer(System.get_env("PORT", "4000"))

  config :canary, CanaryWeb.Endpoint,
    url: [host: host, port: 443, scheme: "https"],
    http: [ip: {0, 0, 0, 0, 0, 0, 0, 0}, port: port],
    secret_key_base: secret_key_base

  dashboard_password_hash =
    case System.get_env("DASHBOARD_PASSWORD") do
      nil ->
        IO.puts("[canary] WARNING: DASHBOARD_PASSWORD not set — dashboard is publicly accessible")
        nil

      "" ->
        raise "DASHBOARD_PASSWORD must not be empty"

      password ->
        Bcrypt.hash_pwd_salt(password)
    end

  config :canary,
    api_key_salt: System.get_env("API_KEY_SALT", secret_key_base),
    allow_private_targets: System.get_env("ALLOW_PRIVATE_TARGETS", "false") == "true",
    error_retention_days: String.to_integer(System.get_env("ERROR_RETENTION_DAYS", "30")),
    check_retention_days: String.to_integer(System.get_env("CHECK_RETENTION_DAYS", "7")),
    dashboard_password_hash: dashboard_password_hash
end
