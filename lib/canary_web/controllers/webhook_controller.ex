defmodule CanaryWeb.WebhookController do
  use CanaryWeb, :controller

  alias Canary.{ID, Repo}
  alias Canary.Schemas.Webhook
  alias Canary.Webhooks.EventTypes
  import Ecto.Query

  def index(conn, _params) do
    webhooks = from(w in Webhook, order_by: w.created_at) |> Canary.Repos.read_repo().all()

    json(conn, %{
      webhooks:
        Enum.map(webhooks, fn w ->
          %{
            id: w.id,
            url: w.url,
            events: Webhook.event_list(w),
            active: Webhook.active?(w),
            created_at: w.created_at
          }
        end)
    })
  end

  def create(conn, params) do
    events = params["events"] || []

    invalid = Enum.reject(events, &EventTypes.valid?/1)

    if invalid != [] do
      CanaryWeb.Plugs.ProblemDetails.render_error(
        conn,
        422,
        "validation_error",
        "Invalid event types: #{Enum.join(invalid, ", ")}"
      )
    else
      secret = Nanoid.generate(32)
      now = DateTime.utc_now() |> DateTime.to_iso8601()

      wh_id = ID.webhook_id()

      attrs = %{
        url: params["url"],
        events: Jason.encode!(events),
        secret: secret,
        created_at: now
      }

      case %Webhook{id: wh_id} |> Webhook.changeset(attrs) |> Repo.insert() do
        {:ok, webhook} ->
          conn
          |> put_status(201)
          |> json(%{
            id: wh_id,
            url: webhook.url,
            events: events,
            secret: secret,
            created_at: now
          })

        {:error, _cs} ->
          CanaryWeb.Plugs.ProblemDetails.render_error(
            conn,
            422,
            "validation_error",
            "Invalid webhook configuration."
          )
      end
    end
  end

  def delete(conn, %{"id" => id}) do
    case Repo.get(Webhook, id) do
      nil ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "Webhook not found."
        )

      webhook ->
        Repo.delete!(webhook)
        conn |> put_status(204) |> text("")
    end
  end

  def test(conn, %{"id" => id}) do
    case Canary.Repos.read_repo().get(Webhook, id) do
      nil ->
        CanaryWeb.Plugs.ProblemDetails.render_error(
          conn,
          404,
          "not_found",
          "Webhook not found."
        )

      webhook ->
        payload = %{
          event: "canary.ping",
          message: "Webhook test from Canary",
          test: true,
          timestamp: DateTime.utc_now() |> DateTime.to_iso8601()
        }

        case Canary.Workers.WebhookDelivery.deliver_test(webhook, payload, "canary.ping") do
          :ok -> json(conn, %{status: "delivered"})
          {:error, reason} -> json(conn, %{status: "failed", reason: reason})
        end
    end
  end
end
