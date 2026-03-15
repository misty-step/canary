defmodule CanaryTriageWeb.WebhookController do
  use CanaryTriageWeb, :controller

  require Logger

  def receive(conn, params) do
    signature = get_req_header(conn, "x-signature") |> List.first() || ""
    event = get_req_header(conn, "x-event") |> List.first() || "unknown"
    raw_body = conn.assigns[:raw_body] || Jason.encode!(params)

    Logger.info("Received webhook: #{event}")

    case CanaryTriage.Dispatch.handle(raw_body, params, signature) do
      {:ok, issue} ->
        json(conn, %{
          status: "created",
          issue_number: issue["number"],
          issue_url: issue["html_url"]
        })

      {:error, :invalid_signature} ->
        conn
        |> put_status(401)
        |> json(%{
          type: "https://canary.dev/problems/invalid-signature",
          title: "Invalid Signature",
          status: 401,
          detail: "HMAC signature verification failed."
        })

      {:error, reason} ->
        Logger.error("Webhook processing failed: #{inspect(reason)}")

        conn
        |> put_status(500)
        |> json(%{
          type: "https://canary.dev/problems/processing-failed",
          title: "Processing Failed",
          status: 500,
          detail: "Webhook dispatch failed."
        })
    end
  end
end
