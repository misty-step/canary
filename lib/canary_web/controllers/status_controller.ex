defmodule CanaryWeb.StatusController do
  use CanaryWeb, :controller

  def index(conn, _params) do
    json(conn, Canary.Status.combined())
  end
end
