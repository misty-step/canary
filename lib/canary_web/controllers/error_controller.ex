defmodule CanaryWeb.ErrorController do
  use CanaryWeb, :controller

  alias Canary.Errors.Ingest

  @max_body_size 102_400

  def create(conn, params) do
    with :ok <- check_body_size(conn) do
      case Ingest.ingest(params) do
        {:ok, result} ->
          conn |> put_status(201) |> json(result)

        {:error, :validation_error, errors} when is_list(errors) ->
          CanaryWeb.Plugs.ProblemDetails.render_error(
            conn,
            422,
            "validation_error",
            "Request body has invalid fields.",
            %{errors: Map.new(errors)}
          )

        {:error, :validation_error, errors} when is_map(errors) ->
          CanaryWeb.Plugs.ProblemDetails.render_error(
            conn,
            422,
            "validation_error",
            "Request body has invalid fields.",
            %{errors: errors}
          )

        {:error, :payload_too_large, detail} ->
          CanaryWeb.Plugs.ProblemDetails.render_error(
            conn,
            413,
            "payload_too_large",
            detail
          )

        {:error, _} ->
          CanaryWeb.Plugs.ProblemDetails.render_error(
            conn,
            500,
            "internal_error",
            "An unexpected error occurred."
          )
      end
    end
  end

  defp check_body_size(conn) do
    case get_req_header(conn, "content-length") do
      [len] ->
        if String.to_integer(len) > @max_body_size do
          CanaryWeb.Plugs.ProblemDetails.render_error(
            conn,
            413,
            "payload_too_large",
            "Request body exceeds 100KB limit."
          )
        else
          :ok
        end

      _ ->
        :ok
    end
  end
end
