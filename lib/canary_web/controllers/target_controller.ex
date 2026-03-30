defmodule CanaryWeb.TargetController do
  use CanaryWeb, :controller

  alias Canary.Health.Manager
  alias Canary.Health.SSRFGuard

  def index(conn, _params) do
    targets = Manager.list_targets()
    json(conn, %{targets: Enum.map(targets, &target_json/1)})
  end

  def create(conn, params) do
    allow_private =
      params["allow_private"] == true or
        Application.get_env(:canary, :allow_private_targets, false)

    case SSRFGuard.validate_url(params["url"], allow_private) do
      :ok ->
        headers = if params["headers"], do: Jason.encode!(params["headers"]), else: nil

        attrs =
          params
          |> Map.put("headers", headers)

        case Manager.add_target(attrs) do
          {:ok, target} ->
            conn |> put_status(201) |> json(target_json(target))

          {:error, changeset} ->
            CanaryWeb.Plugs.ProblemDetails.render_error(
              conn,
              422,
              "validation_error",
              "Invalid target configuration.",
              %{errors: format_errors(changeset)}
            )
        end

      {:error, reason} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          422,
          "validation_error",
          "Invalid URL: #{reason}"
        )
    end
  end

  def delete(conn, %{"id" => id}) do
    case Manager.remove_target(id) do
      {:ok, _} ->
        conn |> put_status(204) |> text("")

      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "Target not found."
        )
    end
  end

  def pause(conn, %{"id" => id}) do
    case Manager.pause_target(id) do
      :ok ->
        json(conn, %{status: "paused"})

      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "Target not found."
        )
    end
  end

  def resume(conn, %{"id" => id}) do
    case Manager.resume_target(id) do
      :ok ->
        json(conn, %{status: "resumed"})

      {:error, :not_found} ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "Target not found."
        )
    end
  end

  defp target_json(target) do
    %{
      id: target.id,
      name: target.name,
      service: Canary.Schemas.Target.service_name(target),
      url: target.url,
      method: target.method,
      interval_ms: target.interval_ms,
      timeout_ms: target.timeout_ms,
      expected_status: target.expected_status,
      active: target.active == 1,
      created_at: target.created_at
    }
  end

  defp format_errors(%Ecto.Changeset{} = cs) do
    Ecto.Changeset.traverse_errors(cs, fn {msg, opts} ->
      Regex.replace(~r"%{(\w+)}", msg, fn _, key ->
        opts |> Keyword.get(String.to_existing_atom(key), key) |> to_string()
      end)
    end)
  end
end
