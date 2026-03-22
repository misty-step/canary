defmodule CanaryWeb.StatusController do
  use CanaryWeb, :controller

  def index(conn, _params) do
    {:ok, status} = Canary.Status.combined()
    json(conn, status)
  end
end
