defmodule CanaryTriage.Dispatch do
  @moduledoc """
  Webhook dispatch pipeline: verify -> enrich -> synthesize -> create issue.

  Deep module: single public function hides the full pipeline.
  """

  require Logger

  @spec handle(binary(), map(), String.t()) :: {:ok, map()} | {:error, term()}
  def handle(raw_body, payload, signature) do
    secret = Application.get_env(:canary_triage, :webhook_secret)

    with :ok <- CanaryTriage.Webhook.verify(raw_body, secret, signature),
         service <- extract_service(payload),
         detail <- enrich(payload),
         {:ok, issue} <- CanaryTriage.Synthesizer.synthesize(payload, detail),
         {:ok, gh_issue} <- CanaryTriage.GitHub.create_issue(service, issue) do
      Logger.info("Dispatched #{payload["event"]} for #{service} -> issue ##{gh_issue["number"]}")
      {:ok, gh_issue}
    else
      {:error, :invalid_signature} ->
        Logger.warning("Rejected webhook: invalid HMAC signature")
        {:error, :invalid_signature}

      {:error, reason} ->
        Logger.error("Dispatch failed: #{inspect(reason)}")
        {:error, reason}
    end
  end

  defp extract_service(payload) do
    get_in(payload, ["error", "service"]) ||
      get_in(payload, ["target", "name"]) ||
      "unknown"
  end

  defp enrich(%{"event" => "error." <> _, "error" => %{"id" => error_id}}) do
    case CanaryTriage.CanaryClient.fetch_error_detail(error_id) do
      {:ok, detail} -> detail
      {:error, _} -> nil
    end
  end

  defp enrich(_payload), do: nil
end
