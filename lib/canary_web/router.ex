defmodule CanaryWeb.Router do
  use CanaryWeb, :router

  pipeline :api do
    plug :accepts, ["json", "csv"]

    plug Plug.Parsers,
      parsers: [:json],
      pass: ["application/json"],
      json_decoder: Jason,
      length: 102_400
  end

  pipeline :authenticated do
    plug CanaryWeb.Plugs.Auth
  end

  pipeline :ingest_rate_limit do
    plug CanaryWeb.Plugs.RateLimit, type: :ingest
  end

  pipeline :query_rate_limit do
    plug CanaryWeb.Plugs.RateLimit, type: :query
  end

  pipeline :browser do
    plug :accepts, ["html"]
    plug :fetch_session
    plug :fetch_live_flash
    plug :put_root_layout, html: {CanaryWeb.Layouts, :root}
    plug :protect_from_forgery
    plug :put_secure_browser_headers
  end

  # Public health endpoints (no auth)
  scope "/", CanaryWeb do
    pipe_through :api

    get "/healthz", HealthController, :healthz
    get "/readyz", HealthController, :readyz
  end

  # Dashboard login (outside live_session — no on_mount gate)
  scope "/dashboard", CanaryWeb do
    pipe_through :browser

    live "/login", LoginLive, :index
    post "/login", LoginController, :create
  end

  # Dashboard (password-gated via on_mount hook)
  scope "/dashboard", CanaryWeb do
    pipe_through :browser

    live_session :dashboard, on_mount: [CanaryWeb.DashboardAuth] do
      live "/", DashboardLive, :index
      live "/errors", ErrorsLive, :index
      live "/errors/:id", ErrorDetailLive, :show
    end
  end

  # Authenticated API
  scope "/api/v1", CanaryWeb do
    pipe_through [:api, :authenticated]

    # Error ingest
    scope "/" do
      pipe_through :ingest_rate_limit
      post "/errors", ErrorController, :create
    end

    # Query API
    scope "/" do
      pipe_through :query_rate_limit
      get "/query", QueryController, :query
      get "/errors/:id", QueryController, :show
      get "/report", ReportController, :index
      get "/timeline", TimelineController, :index
      get "/status", StatusController, :index
      get "/health-status", HealthController, :status
      get "/targets/:id/checks", HealthController, :target_checks
    end

    # Admin: targets
    get "/targets", TargetController, :index
    post "/targets", TargetController, :create
    delete "/targets/:id", TargetController, :delete
    post "/targets/:id/pause", TargetController, :pause
    post "/targets/:id/resume", TargetController, :resume

    # Admin: webhooks
    get "/webhooks", WebhookController, :index
    post "/webhooks", WebhookController, :create
    delete "/webhooks/:id", WebhookController, :delete
    post "/webhooks/:id/test", WebhookController, :test

    # Admin: API keys
    get "/keys", KeyController, :index
    post "/keys", KeyController, :create
    post "/keys/:id/revoke", KeyController, :revoke
  end
end
