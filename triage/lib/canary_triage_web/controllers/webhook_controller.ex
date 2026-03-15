defmodule CanaryTriageWeb.WebhookController do
  use CanaryTriageWeb, :controller

  require Logger

  @doc """
  Receives Canary webhook events, dispatches through the pipeline:
  verify HMAC -> enrich via Canary API -> LLM synthesis -> GitHub issue.

  Responds 200 immediately. Processing is synchronous but fast
  (LLM call is the bottleneck at ~1-3s).
  """
  def receive(conn, params) do
    signature = get_req_header(conn, "x-signature") |> List.first() || ""
    event = get_req_header(conn, "x-event") |> List.first() || "unknown"

    Logger.info("Received webhook: #{event}")

    # Read raw body for HMAC verification
    # Plug.Parsers already consumed it, so we re-encode
    raw_body = Jason.encode!(params)

    case CanaryTriage.Dispatch.handle(raw_body, params, signature) do
      {:ok, issue} ->
        json(conn, %{
          status: "created",
          issue_number: issue["number"],
          issue_url: issue["html_url"]
        })

      {:error, :invalid_signature} ->
        conn |> put_status(401) |> json(%{error: "invalid signature"})

      {:error, reason} ->
        Logger.error("Webhook processing failed: #{inspect(reason)}")
        conn |> put_status(500) |> json(%{error: "processing failed"})
    end
  end
end
