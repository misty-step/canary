defmodule CanaryWeb.CheckInController do
  use CanaryWeb, :controller

  alias Canary.Monitors

  def create(conn, params) do
    case Monitors.process_check_in(params) do
      {:ok, %{check_in: check_in, state: state}} ->
        conn
        |> put_status(201)
        |> json(%{
          monitor_id: check_in.monitor_id,
          check_in_id: check_in.id,
          state: state.state,
          observed_at: check_in.observed_at,
          sequence: state.sequence
        })

      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "Monitor not found."
        )

      {:error, :invalid_timestamp} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid observed_at timestamp.",
          %{errors: %{observed_at: ["must be an ISO8601 timestamp"]}}
        )

      {:error, {:validation, errors}} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid check-in payload.",
          %{errors: errors}
        )
    end
  end
end
