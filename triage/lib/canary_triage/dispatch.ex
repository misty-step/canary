defmodule CanaryTriage.Dispatch do
  @moduledoc """
  Webhook dispatch pipeline: verify -> route by event type -> act.

  Deep module: single public function hides event-aware dispatch.
  """

  require Logger

  @spec handle(binary(), map(), String.t()) :: {:ok, map() | :noop} | {:error, term()}
  def handle(raw_body, payload, signature) do
    secret = Application.get_env(:canary_triage, :webhook_secret)

    with :ok <- CanaryTriage.Webhook.verify(raw_body, secret, signature) do
      dispatch(payload)
    else
      {:error, :invalid_signature} ->
        Logger.warning("Rejected webhook: invalid HMAC signature")
        {:error, :invalid_signature}
    end
  end

  # Health check events: template-based, issue lifecycle management
  defp dispatch(%{"event" => "health_check.recovered"} = payload) do
    handle_recovery(extract_service(payload), payload)
  end

  defp dispatch(%{"event" => "health_check." <> type} = payload)
       when type in ["degraded", "down"] do
    handle_degradation(extract_service(payload), payload)
  end

  # Non-lifecycle health events (tls_expiring, etc.) — acknowledge, don't act
  defp dispatch(%{"event" => "health_check." <> _} = payload) do
    Logger.info("Ignoring non-lifecycle health check event: #{payload["event"]}")
    {:ok, :noop}
  end

  # Error events: LLM synthesis -> create issue (unchanged)
  defp dispatch(%{"event" => "error." <> _} = payload) do
    service = extract_service(payload)
    detail = enrich(payload)

    with {:ok, issue} <- CanaryTriage.Synthesizer.synthesize(payload, detail),
         {:ok, gh_issue} <- CanaryTriage.GitHub.create_issue(service, issue) do
      Logger.info("Dispatched #{payload["event"]} for #{service} -> issue ##{gh_issue["number"]}")
      {:ok, gh_issue}
    else
      {:error, reason} ->
        Logger.error("Dispatch failed for #{payload["event"]}: #{inspect(reason)}")
        {:error, reason}
    end
  end

  defp dispatch(payload) do
    event = payload["event"] || "unknown"
    Logger.warning("Unhandled event type: #{event}")
    {:error, {:unhandled_event, event}}
  end

  defp handle_degradation(service, payload) do
    case CanaryTriage.GitHub.find_open_health_issue(service) do
      {:ok, existing} ->
        comment = CanaryTriage.Synthesizer.build_health_check_comment(payload)

        case CanaryTriage.GitHub.comment_on_issue(service, existing["number"], comment) do
          {:ok, _} ->
            Logger.info("Commented on health issue ##{existing["number"]} for #{service}")
            {:ok, existing}

          {:error, _} = error ->
            error
        end

      :not_found ->
        {:ok, issue} = CanaryTriage.Synthesizer.build_health_check_issue(payload)

        case CanaryTriage.GitHub.create_issue(service, issue) do
          {:ok, gh_issue} ->
            Logger.info("Created health issue ##{gh_issue["number"]} for #{service}")
            {:ok, gh_issue}

          {:error, _} = error ->
            error
        end

      {:error, _} = error ->
        error
    end
  end

  defp handle_recovery(service, payload) do
    case CanaryTriage.GitHub.find_open_health_issue(service) do
      {:ok, existing} ->
        comment = CanaryTriage.Synthesizer.build_recovery_comment(payload)

        case CanaryTriage.GitHub.close_issue(service, existing["number"], comment) do
          {:ok, _} ->
            Logger.info("Closed health issue ##{existing["number"]} for #{service} (recovered)")
            {:ok, existing}

          {:error, _} = error ->
            error
        end

      :not_found ->
        Logger.info("Recovery for #{service} but no open health issue — no-op")
        {:ok, :noop}

      {:error, _} = error ->
        error
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
