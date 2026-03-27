defmodule CanaryTriageWeb.Router do
  use CanaryTriageWeb, :router

  pipeline :webhook do
    plug :accepts, ["json"]

    plug Plug.Parsers,
      parsers: [:json],
      pass: ["application/json"],
      json_decoder: Jason
  end

  pipeline :api do
    plug :accepts, ["json"]
  end

  scope "/", CanaryTriageWeb do
    pipe_through :api
    get "/healthz", HealthController, :healthz
  end

  scope "/webhooks", CanaryTriageWeb do
    pipe_through :webhook
    post "/canary", WebhookController, :receive
  end
end
