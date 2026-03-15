defmodule CanaryTriage.Synthesizer do
  @moduledoc """
  LLM-powered incident -> GitHub issue synthesis via OpenRouter.

  Single call with JSON mode. The webhook payload + enriched detail
  is sufficient context. The LLM decides priority, writes the narrative,
  suggests investigation steps.
  """

  require Logger

  @openrouter_url "https://openrouter.ai/api/v1/chat/completions"
  @model "google/gemini-3-flash-preview"

  @type issue :: %{
          title: String.t(),
          body: String.t(),
          labels: [String.t()],
          priority: String.t()
        }

  @spec synthesize(map(), map() | nil) :: {:ok, issue()} | {:error, term()}
  def synthesize(webhook_payload, enriched_detail \\ nil) do
    api_key = Application.get_env(:canary_triage, :openrouter_api_key)

    prompt = build_prompt(webhook_payload, enriched_detail)

    body = %{
      model: @model,
      messages: [%{role: "user", content: prompt}],
      response_format: %{type: "json_object"}
    }

    case Req.post(@openrouter_url,
           json: body,
           headers: [{"authorization", "Bearer #{api_key}"}],
           receive_timeout: 30_000,
           finch: CanaryTriage.Finch
         ) do
      {:ok, %{status: 200, body: resp}} ->
        parse_response(resp)

      {:ok, %{status: status, body: resp}} ->
        Logger.error("OpenRouter API error: #{status} #{inspect(resp)}")
        {:error, {:openrouter, status}}

      {:error, reason} ->
        Logger.error("OpenRouter API request failed: #{inspect(reason)}")
        {:error, reason}
    end
  end

  defp build_prompt(payload, detail) do
    event = payload["event"] || "unknown"
    service = get_in(payload, ["error", "service"]) || get_in(payload, ["target", "name"]) || "unknown"

    context =
      case {event, detail} do
        {"error." <> _, %{"stack_trace" => st}} when is_binary(st) ->
          """
          Error class: #{get_in(payload, ["error", "error_class"])}
          Message: #{get_in(payload, ["error", "message"])}
          Service: #{service}
          Severity: #{get_in(payload, ["error", "severity"])}
          Group hash: #{get_in(payload, ["error", "group_hash"])}
          Occurrences: #{get_in(detail, ["group", "total_count"]) || 1}
          First seen: #{get_in(detail, ["group", "first_seen_at"]) || payload["timestamp"]}
          Last seen: #{get_in(detail, ["group", "last_seen_at"]) || payload["timestamp"]}
          Environment: #{detail["environment"] || "production"}

          Stack trace:
          #{st}

          Context: #{inspect(detail["context"])}
          """

        {"error." <> _, _} ->
          """
          Error class: #{get_in(payload, ["error", "error_class"])}
          Message: #{get_in(payload, ["error", "message"])}
          Service: #{service}
          Severity: #{get_in(payload, ["error", "severity"])}
          """

        {"health_check." <> _, _} ->
          """
          Target: #{get_in(payload, ["target", "name"])} (#{get_in(payload, ["target", "url"])})
          State: #{payload["state"]}
          Previous state: #{payload["previous_state"]}
          Consecutive failures: #{payload["consecutive_failures"]}
          Last success: #{payload["last_success_at"]}
          Last check result: #{get_in(payload, ["last_check", "result"])}
          Last check status: #{get_in(payload, ["last_check", "status_code"])}
          Last check latency: #{get_in(payload, ["last_check", "latency_ms"])}ms
          """

        _ ->
          inspect(payload, pretty: true)
      end

    """
    You are an expert incident responder creating a GitHub issue from an automated observability alert.

    Event type: #{event}
    Service: #{service}
    Timestamp: #{payload["timestamp"]}

    #{context}

    Create a GitHub issue. Respond with JSON matching this exact shape:
    {"title": "...", "body": "...", "labels": ["..."], "priority": "critical|high|medium|low"}

    Rules:
    1. Title: concise, specific (include error class or target name)
    2. Body: GitHub-flavored markdown with summary, impact, technical details, investigation steps. Put raw data in a <details> block.
    3. Labels from: bug, critical, high-priority, medium-priority, low-priority, health-check, error, regression, tls
    4. Priority: critical (production down / data loss), high (degraded service), medium (intermittent / new error class), low (warning / info severity)

    Be specific and actionable. Do not hallucinate information not in the data above.
    """
  end

  defp parse_response(resp) do
    text =
      get_in(resp, ["choices", Access.at(0), "message", "content"])

    case text do
      nil ->
        {:error, :no_content}

      json_str ->
        case Jason.decode(json_str) do
          {:ok, %{"title" => _, "body" => _, "labels" => _, "priority" => _} = issue} ->
            {:ok, issue}

          {:ok, other} ->
            Logger.error("Unexpected LLM response shape: #{inspect(other)}")
            {:error, :bad_shape}

          {:error, reason} ->
            Logger.error("Failed to parse LLM JSON: #{inspect(reason)}")
            {:error, :json_parse}
        end
    end
  end
end
