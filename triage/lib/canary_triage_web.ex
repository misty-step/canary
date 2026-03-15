defmodule CanaryTriageWeb do
  @moduledoc false

  def static_paths, do: ~w(robots.txt)

  def router do
    quote do
      use Phoenix.Router, helpers: false
      import Plug.Conn
      import Phoenix.Controller
    end
  end

  def controller do
    quote do
      use Phoenix.Controller, formats: [:json]
      import Plug.Conn
      unquote(verified_routes())
    end
  end

  def verified_routes do
    quote do
      use Phoenix.VerifiedRoutes,
        endpoint: CanaryTriageWeb.Endpoint,
        router: CanaryTriageWeb.Router,
        statics: CanaryTriageWeb.static_paths()
    end
  end

  defmacro __using__(which) when is_atom(which) do
    apply(__MODULE__, which, [])
  end
end
