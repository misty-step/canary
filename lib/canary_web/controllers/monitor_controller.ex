defmodule CanaryWeb.MonitorController do
  use CanaryWeb, :controller

  alias Canary.ChangesetErrors
  alias Canary.MonitorResponse
  alias Canary.Monitors

  def index(conn, _params) do
    monitors = Monitors.list_monitors()
    json(conn, %{monitors: Enum.map(monitors, &MonitorResponse.render/1)})
  end

  def create(conn, params) do
    case Monitors.add_monitor(params) do
      {:ok, monitor} ->
        conn |> put_status(201) |> json(MonitorResponse.render(monitor))

      {:error, changeset} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid monitor configuration.",
          %{errors: ChangesetErrors.format(changeset)}
        )
    end
  end

  def delete(conn, %{"id" => id}) do
    case Monitors.remove_monitor(id) do
      {:ok, _monitor} ->
        conn |> put_status(204) |> text("")

      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "Monitor not found."
        )
    end
  end
end
